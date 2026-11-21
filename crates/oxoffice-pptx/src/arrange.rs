//! Arranging shapes: z-order moves, grouping and ungrouping, aligning and distributing, connector glue, and position, size, rotation and flip edits.

use super::*;

impl Presentation {
    /// Moves one immediate slide child so that it ends up at z-order index
    /// `to_index`, where later children draw on top.
    ///
    /// Animations, connector glue, and relationships refer to shape ids, so
    /// they follow the moved shape. Unmodelled shape-tree members keep their
    /// place among the other children.
    pub fn move_shape(
        &mut self,
        slide_index: usize,
        from_index: usize,
        to_index: usize,
    ) -> Result<()> {
        self.move_shape_at(ShapeOwner::Slide(slide_index), from_index, to_index)
    }

    /// Changes direct shape order on one actual slide, layout or master.
    pub fn move_shape_at(
        &mut self,
        owner: ShapeOwner,
        from_index: usize,
        to_index: usize,
    ) -> Result<()> {
        const OPERATION: &str = "move shape";
        let count = self.owner_data(owner)?.shape_tree.children.len();
        if let Some(index) = [from_index, to_index]
            .into_iter()
            .find(|index| *index >= count)
        {
            return Err(invalid_slide_mutation(
                OPERATION,
                format!("shape index {index} is out of range for {count} shapes"),
            ));
        }
        self.owner_data_mut(owner)?
            .shape_tree
            .move_child(from_index, to_index)
            .map_err(|error| invalid_shape_mutation(OPERATION, error.to_string()))
    }
}

/// The edge or centre line [`ShapesMut::align`] lines shapes up on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShapeAlignment {
    Left,
    Center,
    Right,
    Top,
    Middle,
    Bottom,
}

/// What [`ShapesMut::align`] and [`ShapesMut::distribute`] measure against:
/// the bounding box of the shapes given, or the slide.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArrangeReference {
    Selection,
    Slide,
}

/// The axis [`ShapesMut::distribute`] spaces shapes along.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DistributeDirection {
    Horizontal,
    Vertical,
}

/// One end of a connector, `a:stCxn` for `Begin` and `a:endCxn` for `End`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectorEnd {
    Begin,
    End,
}

/// An affine map in PDF order: `x' = a x + c y + e`, `y' = b x + d y + f`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Affine([f64; 6]);

impl Affine {
    const IDENTITY: Self = Self([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    const fn translate(x: f64, y: f64) -> Self {
        Self([1.0, 0.0, 0.0, 1.0, x, y])
    }

    const fn scale(x: f64, y: f64) -> Self {
        Self([x, 0.0, 0.0, y, 0.0, 0.0])
    }

    /// A clockwise rotation on the y-down slide plane.
    fn rotate(degrees: f64) -> Self {
        let (sin, cos) = degrees.to_radians().sin_cos();
        Self([cos, sin, -sin, cos, 0.0, 0.0])
    }

    /// Applies `self`, then `next`.
    fn then(self, next: Self) -> Self {
        let [a, b, c, d, e, f] = self.0;
        let [na, nb, nc, nd, ne, nf] = next.0;
        Self([
            na * a + nc * b,
            nb * a + nd * b,
            na * c + nc * d,
            nb * c + nd * d,
            na * e + nc * f + ne,
            nb * e + nd * f + nf,
        ])
    }

    fn apply(self, (x, y): (f64, f64)) -> (f64, f64) {
        let [a, b, c, d, e, f] = self.0;
        (a * x + c * y + e, b * x + d * y + f)
    }

    fn inverse(self) -> Option<Self> {
        let [a, b, c, d, e, f] = self.0;
        let determinant = a * d - b * c;
        if !self.0.into_iter().all(f64::is_finite)
            || !determinant.is_finite()
            || determinant.abs() < f64::EPSILON
        {
            return None;
        }
        let (ia, ib, ic, id) = (
            d / determinant,
            -b / determinant,
            -c / determinant,
            a / determinant,
        );
        Some(Self([
            ia,
            ib,
            ic,
            id,
            -(ia * e + ic * f),
            -(ib * e + id * f),
        ]))
    }
}

/// Flips about `centre`, then rotates about it, as PowerPoint orients a box.
fn orientation_about(transform: &CT_Transform2D, (x, y): (f64, f64)) -> Affine {
    let flip = |flipped: bool| if flipped { -1.0 } else { 1.0 };
    Affine::translate(-x, -y)
        .then(Affine::scale(
            flip(transform.flip_horizontal),
            flip(transform.flip_vertical),
        ))
        .then(Affine::rotate(transform.rotation.to_degrees()))
        .then(Affine::translate(x, y))
}

fn box_centre(offset: CT_Point2D, extent: CT_PositiveSize2D) -> (f64, f64) {
    (
        offset.x.0 as f64 + extent.cx.0 as f64 / 2.0,
        offset.y.0 as f64 + extent.cy.0 as f64 / 2.0,
    )
}

/// Maps a shape's own box, from `(0, 0)` to its extent, to its parent.
fn box_to_parent(transform: &CT_Transform2D) -> Option<Affine> {
    let (offset, extent) = (transform.offset?, transform.extent?);
    Some(
        Affine::translate(offset.x.0 as f64, offset.y.0 as f64)
            .then(orientation_about(transform, box_centre(offset, extent))),
    )
}

/// The member-space scale of a group on each axis, one where the group has
/// no child extent or a zero one, as the renderer reads it.
fn group_scale(transform: &CT_Transform2D) -> (f64, f64) {
    let ratio = |extent: Option<i64>, child: Option<i64>| match (extent, child) {
        (Some(extent), Some(child)) if child != 0 => extent as f64 / child as f64,
        _ => 1.0,
    };
    (
        ratio(
            transform.extent.map(|extent| extent.cx.0),
            transform.child_extent.map(|extent| extent.cx.0),
        ),
        ratio(
            transform.extent.map(|extent| extent.cy.0),
            transform.child_extent.map(|extent| extent.cy.0),
        ),
    )
}

/// Maps a group's member space to the group's parent.
fn members_to_parent(transform: Option<&CT_Transform2D>) -> Affine {
    let Some(transform) = transform else {
        return Affine::IDENTITY;
    };
    let (Some(offset), Some(extent)) = (transform.offset, transform.extent) else {
        return Affine::IDENTITY;
    };
    let child_offset = transform.child_offset.unwrap_or(offset);
    let (scale_x, scale_y) = group_scale(transform);
    Affine::translate(-(child_offset.x.0 as f64), -(child_offset.y.0 as f64))
        .then(Affine::scale(scale_x, scale_y))
        .then(Affine::translate(offset.x.0 as f64, offset.y.0 as f64))
        .then(orientation_about(transform, box_centre(offset, extent)))
}

/// Maps the member space of the group that `group_path` names, or of the
/// slide for an empty path, to the slide.
fn members_to_slide(children: &[ShapeTreeChild], group_path: &[usize]) -> Option<Affine> {
    let mut maps = Vec::with_capacity(group_path.len());
    let mut level = children;
    for index in group_path {
        let ShapeTreeChild::GroupShape(group) = level.get(*index)? else {
            return None;
        };
        let transform = group.group_transform()?;
        let child_extent = transform.child_extent?;
        if child_extent.cx.0 <= 0 || child_extent.cy.0 <= 0 {
            return None;
        }
        maps.push(members_to_parent(Some(transform)));
        level = &group.children;
    }
    Some(
        maps.into_iter()
            .rev()
            .fold(Affine::IDENTITY, |inner, outer| inner.then(outer)),
    )
}

pub(super) fn checked_geometry_emu(value: f64) -> Result<Emu> {
    let rounded = value.round();
    if !rounded.is_finite() || rounded < i64::MIN as f64 || rounded >= -(i64::MIN as f64) {
        return Err(invalid_shape_mutation(
            "shape geometry",
            "nonfinite or out of range EMU",
        ));
    }
    Ok(Emu(rounded as i64))
}

/// The members of a slide's own shape tree or of one group on it.
pub(super) enum Members<'t> {
    Tree(&'t mut CT_ShapeTree),
    Group(&'t mut CT_GroupShape),
}

impl<'t> Members<'t> {
    pub(super) fn at(tree: &'t mut CT_ShapeTree, group: &[usize]) -> Option<Self> {
        if group.is_empty() {
            Some(Self::Tree(tree))
        } else {
            group_at_mut(&mut tree.children, group).map(Self::Group)
        }
    }

    pub(super) fn children(&self) -> &[ShapeTreeChild] {
        match self {
            Self::Tree(tree) => &tree.children,
            Self::Group(group) => &group.children,
        }
    }

    fn children_mut(&mut self) -> &mut Vec<ShapeTreeChild> {
        match self {
            Self::Tree(tree) => &mut tree.children,
            Self::Group(group) => &mut group.children,
        }
    }

    fn group_span(&mut self, start: usize, count: usize, id: u32, name: &str) -> Result<()> {
        match self {
            Self::Tree(tree) => tree
                .group_member_span(start, count, id, name)
                .map_err(|error| invalid_shape_construction("group shapes", error)),
            Self::Group(group) => group
                .group_member_span(start, count, id, name)
                .map_err(|error| invalid_shape_construction("group shapes", error)),
        }
    }
    fn replace_group(
        &mut self,
        index: usize,
        placed: Vec<ShapeTreeChild>,
        identity: bool,
    ) -> Result<()> {
        match self {
            Self::Tree(tree) => tree
                .replace_group_member(index, placed, identity)
                .map_err(|error| invalid_shape_construction("ungroup shapes", error)),
            Self::Group(group) => group
                .replace_group_member(index, placed, identity)
                .map_err(|error| invalid_shape_construction("ungroup shapes", error)),
        }
    }
}

/// Returns `indices` ascending, rejecting an empty list, a repeat, and an
/// index outside the `count` members.
fn sorted_indices(operation: &'static str, indices: &[usize], count: usize) -> Result<Vec<usize>> {
    let mut sorted = indices.to_vec();
    sorted.sort_unstable();
    if sorted.is_empty() {
        return Err(invalid_shape_mutation(operation, "no shapes were given"));
    }
    if let Some(index) = sorted.iter().find(|index| **index >= count) {
        return Err(invalid_shape_mutation(
            operation,
            format!("shape index {index} is out of range for {count} shapes"),
        ));
    }
    if let Some(pair) = sorted.windows(2).find(|pair| pair[0] == pair[1]) {
        return Err(invalid_shape_mutation(
            operation,
            format!("shape index {} is given twice", pair[0]),
        ));
    }
    Ok(sorted)
}

impl ShapesMut<'_> {
    fn tree(&mut self) -> &mut CT_ShapeTree {
        &mut self
            .presentation
            .owner_data_mut(self.owner)
            .expect("validated owner")
            .shape_tree
    }

    /// Moves existing shapes of this collection into a new group and returns
    /// the group, as python-pptx `add_group_shape(shapes)` does.
    ///
    /// `indices` name z-order positions in this collection, in any order.
    /// The members keep their order among themselves and their place on the
    /// slide: the group's `a:chOff` and `a:chExt` equal its `a:off` and
    /// `a:ext`, the union of the members' boxes. The group takes the z-order
    /// place of the selected source span. Interleaved opaque members move
    /// inside its identity coordinate map, retaining source order against
    /// selected members and unselected neighbors. Shape ids do not change, so animations
    /// and glued connectors keep their targets. A placeholder, which
    /// PowerPoint does not group, a shape without a position and size of its
    /// own, and a repeated or missing index are rejected without change.
    fn group_in_candidate(&mut self, indices: &[usize]) -> Result<ShapeMut<'_>> {
        const OPERATION: &str = "group shapes";
        let group_path = self.group.clone();
        let tree = self.tree();
        oxoffice_pptx_oxml::shape_tree::validate_shape_nesting(&tree.children)
            .map_err(|error| invalid_shape_mutation(OPERATION, error.to_string()))?;
        let id = ShapeIdAllocator::scan(tree).allocate();
        let mut members = Members::at(tree, &group_path)
            .ok_or_else(|| invalid_shape_mutation(OPERATION, "the collection is not a group"))?;
        let sorted = sorted_indices(OPERATION, indices, members.children().len())?;
        if sorted.windows(2).any(|pair| pair[1] != pair[0] + 1) {
            return Err(invalid_shape_mutation(
                OPERATION,
                "group selection must be contiguous",
            ));
        }
        for index in &sorted {
            let child = &members.children()[*index];
            if shape_placeholder(child).is_some() {
                return Err(invalid_shape_mutation(
                    OPERATION,
                    format!(
                        "shape {index} is a placeholder, which PowerPoint does not group; \
                         remove it from the list"
                    ),
                ));
            }
            if member_box(child).is_none() {
                return Err(invalid_shape_mutation(
                    OPERATION,
                    format!("shape {index} has no position and size of its own"),
                ));
            }
        }
        let selected_depth = sorted
            .iter()
            .try_fold(0usize, |deepest, index| {
                oxoffice_pptx_oxml::shape_tree::validate_shape_nesting(std::slice::from_ref(
                    &members.children()[*index],
                ))
                .map(|depth| deepest.max(depth))
            })
            .map_err(|error| invalid_shape_mutation(OPERATION, error.to_string()))?;
        let depth = group_path.len() + 1 + selected_depth;
        if depth > oxoffice_pptx_oxml::MAX_PRESENTATION_NESTING {
            return Err(invalid_shape_mutation(
                OPERATION,
                format!(
                    "group and alternate content nesting exceeds limit {}, offending depth {depth}",
                    oxoffice_pptx_oxml::MAX_PRESENTATION_NESTING
                ),
            ));
        }
        let place = sorted[0];
        members.group_span(place, sorted.len(), id, &format!("Group {id}"))?;
        let ShapeTreeChild::GroupShape(group) = &mut members.children_mut()[place] else {
            unreachable!("checked model span creates one group");
        };
        fit_group_to_members(group, false);
        let mut path = group_path;
        path.push(place);
        Ok(owned_shape_mut(self.presentation, self.owner, path))
    }

    /// Replaces the group at `index` by its members and returns their new
    /// z-order range.
    ///
    /// Each member keeps its place on the slide: the group's scale, flips,
    /// and rotation move into the member's own transform, as PowerPoint's
    /// Ungroup does. A member that is scaled while rotated by other than a
    /// multiple of 90 degrees is refused if the composition introduces
    /// skew. Opaque members move unchanged under an identity map, while an
    /// unsupported nonidentity opaque composition or nontransferable group
    /// metadata refuses before publication. A table, chart, or other graphic frame
    /// cannot rotate or flip, so a group holding one that is not merely
    /// moved is refused, as is a group that slide animations target.
    /// Connectors glued to the group itself are released, and connectors
    /// glued to members stay glued.
    fn ungroup_in_candidate(&mut self, index: usize) -> Result<std::ops::Range<usize>> {
        const OPERATION: &str = "ungroup shapes";
        let group_path = self.group.clone();
        let timing = match self.owner {
            ShapeOwner::Slide(i) => self.presentation.slides[i].slide.timing.clone(),
            ShapeOwner::Layout(i) => self.presentation.layouts[i].layout.timing.clone(),
            ShapeOwner::Master(i) => self.presentation.masters[i]
                .master
                .as_ref()
                .expect("validated owner")
                .timing
                .clone(),
        };
        let common = self.presentation.owner_data_mut(self.owner)?;
        let members = Members::at(&mut common.shape_tree, &group_path)
            .ok_or_else(|| invalid_shape_mutation(OPERATION, "the collection is not a group"))?;
        let count = members.children().len();
        let child = members.children().get(index).ok_or_else(|| {
            invalid_shape_mutation(
                OPERATION,
                format!("shape index {index} is out of range for {count} shapes"),
            )
        })?;
        let ShapeTreeChild::GroupShape(group) = child else {
            return Err(Error::UnsupportedShapeMutation {
                operation: OPERATION,
                shape_kind: shape_kind(child),
            });
        };
        let group_id = child.non_visual_id();
        if let (Some(timing), Some(id)) = (&timing, group_id)
            && timing.references_shape(id)
        {
            return Err(invalid_shape_mutation(
                OPERATION,
                format!("slide animations target group id {id}; remove them first"),
            ));
        }
        let transform = group.group_transform().cloned().unwrap_or_default();
        let mut placed = group.children.clone();
        for (position, member) in placed.iter_mut().enumerate() {
            place_in_parent(member, &transform).map_err(|message| {
                invalid_shape_mutation(OPERATION, format!("member {position}: {message}"))
            })?;
        }
        let range = index..index + placed.len();
        let mut members =
            Members::at(&mut common.shape_tree, &group_path).expect("the group path was checked");
        members.replace_group(
            index,
            placed,
            members_to_parent(Some(&transform)) == Affine::IDENTITY,
        )?;
        if let Some(id) = group_id {
            detach_connectors(&mut common.shape_tree.children, &HashSet::from([id]));
        }
        Ok(range)
    }

    /// Lines the shapes at `indices` up on one edge or centre line of their
    /// bounding box, or of the slide, as PowerPoint's Align does.
    ///
    /// Boxes include flips and rotation, so a rotated shape lines up by what
    /// is drawn. A placeholder that inherits its position first receives
    /// that position. `ArrangeReference::Slide` measures in slide
    /// coordinates, including nested same-parent group members through the
    /// inverse parent map.
    fn align_in_candidate(
        &mut self,
        indices: &[usize],
        alignment: ShapeAlignment,
        reference: ArrangeReference,
    ) -> Result<()> {
        const OPERATION: &str = "align shapes";
        let (sorted, bounds, target) = self.arrangement(OPERATION, indices, reference)?;
        let shifts = bounds
            .iter()
            .map(|[left, top, right, bottom]| match alignment {
                ShapeAlignment::Left => (target[0] - left, 0.0),
                ShapeAlignment::Center => ((target[0] + target[2] - left - right) / 2.0, 0.0),
                ShapeAlignment::Right => (target[2] - right, 0.0),
                ShapeAlignment::Top => (0.0, target[1] - top),
                ShapeAlignment::Middle => (0.0, (target[1] + target[3] - top - bottom) / 2.0),
                ShapeAlignment::Bottom => (0.0, target[3] - bottom),
            })
            .collect::<Vec<_>>();
        self.shift(OPERATION, &sorted, &shifts)
    }

    /// Spaces the shapes at `indices` evenly along one axis, as PowerPoint's
    /// Distribute does.
    ///
    /// Shapes are taken in their order along the axis. Relative to the
    /// selection, the first and last stay and the others move so that the
    /// gaps between boxes are equal, which needs three shapes. Relative to
    /// the slide, the first box starts at the slide edge and the last ends
    /// at the other, and a single shape is centred. Boxes include flips and
    /// rotation, as for [`Self::align`].
    fn distribute_in_candidate(
        &mut self,
        indices: &[usize],
        direction: DistributeDirection,
        reference: ArrangeReference,
    ) -> Result<()> {
        const OPERATION: &str = "distribute shapes";
        let (sorted, bounds, target) = self.arrangement(OPERATION, indices, reference)?;
        let (start, end) = match direction {
            DistributeDirection::Horizontal => (0, 2),
            DistributeDirection::Vertical => (1, 3),
        };
        let mut order = (0..sorted.len()).collect::<Vec<_>>();
        order.sort_by(|left, right| bounds[*left][start].total_cmp(&bounds[*right][start]));
        let lengths = bounds
            .iter()
            .map(|bound| bound[end] - bound[start])
            .collect::<Vec<_>>();
        let total = lengths.iter().sum::<f64>();
        let span = target[end] - target[start];
        let mut shifts = vec![(0.0, 0.0); sorted.len()];
        let mut cursor = target[start];
        let gap = if order.len() == 1 {
            cursor += (span - total) / 2.0;
            0.0
        } else {
            (span - total) / (order.len() - 1) as f64
        };
        for position in order {
            let delta = cursor - bounds[position][start];
            shifts[position] = match direction {
                DistributeDirection::Horizontal => (delta, 0.0),
                DistributeDirection::Vertical => (0.0, delta),
            };
            cursor += lengths[position] + gap;
        }
        self.shift(OPERATION, &sorted, &shifts)
    }

    /// Checks the shapes an arrangement moves and returns their sorted
    /// indices, their drawn boxes, and the box they arrange against.
    #[allow(clippy::type_complexity)]
    fn arrangement(
        &mut self,
        operation: &'static str,
        indices: &[usize],
        reference: ArrangeReference,
    ) -> Result<(Vec<usize>, Vec<[f64; 4]>, [f64; 4])> {
        let group_path = self.group.clone();
        let count =
            Members::at(self.tree(), &group_path).map_or(0, |members| members.children().len());
        let sorted = sorted_indices(operation, indices, count)?;
        let minimum = match (operation, reference) {
            ("align shapes", ArrangeReference::Selection) => 2,
            ("distribute shapes", ArrangeReference::Selection) => 3,
            _ => 1,
        };
        if sorted.len() < minimum {
            let reference_name = match reference {
                ArrangeReference::Selection => "selection",
                ArrangeReference::Slide => "slide",
            };
            return Err(invalid_shape_mutation(
                operation,
                format!(
                    "too few selected shapes: {reference_name} reference requires at least {minimum}, got {}",
                    sorted.len()
                ),
            ));
        }
        let parent = members_to_slide(
            &self
                .presentation
                .owner_data(self.owner)?
                .shape_tree
                .children,
            &group_path,
        )
        .ok_or_else(|| invalid_shape_mutation(operation, "missing parent transform"))?;
        if parent.inverse().is_none() {
            return Err(invalid_shape_mutation(
                operation,
                "singular parent transform",
            ));
        }
        for index in &sorted {
            let mut path = self.group.clone();
            path.push(*index);
            self.presentation
                .materialize_owner_geometry(self.owner, &path)?;
        }
        let members = Members::at(self.tree(), &group_path).expect("the group path was checked");
        let bounds = sorted
            .iter()
            .map(|index| {
                let child = &members.children()[*index];
                if matches!(shape_kind(child), ShapeKind::AlternateContent) {
                    return Err(Error::UnsupportedShapeMutation {
                        operation,
                        shape_kind: ShapeKind::AlternateContent,
                    });
                }
                shape_transform(child)
                    .and_then(|t| {
                        let map = box_to_parent(t)?.then(parent);
                        let ext = t.extent?;
                        let points = [
                            (0.0, 0.0),
                            (ext.cx.0 as f64, 0.0),
                            (0.0, ext.cy.0 as f64),
                            (ext.cx.0 as f64, ext.cy.0 as f64),
                        ]
                        .map(|p| map.apply(p));
                        if !points.iter().all(|(x, y)| x.is_finite() && y.is_finite()) {
                            return None;
                        }
                        Some(points.into_iter().fold(
                            [
                                f64::INFINITY,
                                f64::INFINITY,
                                f64::NEG_INFINITY,
                                f64::NEG_INFINITY,
                            ],
                            |b, (x, y)| [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)],
                        ))
                    })
                    .ok_or_else(|| {
                        invalid_shape_mutation(
                            operation,
                            format!("shape {index} has no position and size"),
                        )
                    })
            })
            .collect::<Result<Vec<_>>>()?;
        let target = match reference {
            ArrangeReference::Selection => bounds.iter().fold(
                [
                    f64::INFINITY,
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                    f64::NEG_INFINITY,
                ],
                |[left, top, right, bottom], bound| {
                    [
                        left.min(bound[0]),
                        top.min(bound[1]),
                        right.max(bound[2]),
                        bottom.max(bound[3]),
                    ]
                },
            ),
            ArrangeReference::Slide => {
                let (width, height) = self.presentation.slide_size().ok_or_else(|| {
                    invalid_shape_mutation(operation, "the presentation has no slide size")
                })?;
                [0.0, 0.0, width.0 as f64, height.0 as f64]
            }
        };
        Ok((sorted, bounds, target))
    }

    /// Moves each shape at `sorted` by its `(dx, dy)`.
    fn shift(
        &mut self,
        operation: &'static str,
        sorted: &[usize],
        shifts: &[(f64, f64)],
    ) -> Result<()> {
        let group_path = self.group.clone();
        let inverse = members_to_slide(
            &self
                .presentation
                .owner_data(self.owner)?
                .shape_tree
                .children,
            &group_path,
        )
        .and_then(Affine::inverse)
        .ok_or_else(|| invalid_shape_mutation(operation, "singular parent transform"))?;
        let mut members = Members::at(self.tree(), &group_path)
            .ok_or_else(|| invalid_shape_mutation(operation, "missing group"))?;
        let mut offsets = Vec::with_capacity(sorted.len());
        for (index, (dx, dy)) in sorted.iter().zip(shifts) {
            let offset = shape_transform(&members.children()[*index])
                .and_then(|t| t.offset)
                .ok_or_else(|| invalid_shape_mutation(operation, "missing offset"))?;
            let origin = inverse.apply((0.0, 0.0));
            let point = inverse.apply((*dx, *dy));
            offsets.push(CT_Point2D {
                x: checked_geometry_emu(offset.x.0 as f64 + point.0 - origin.0)?,
                y: checked_geometry_emu(offset.y.0 as f64 + point.1 - origin.1)?,
            });
        }
        for (index, offset) in sorted.iter().zip(offsets) {
            shape_mut(&mut members.children_mut()[*index])
                .transform_mut(operation)?
                .offset = Some(offset);
        }
        Ok(())
    }
}

/// Rewrites one member's transform so that it draws in its group's parent
/// where it drew inside the group of `transform`.
fn place_in_parent(
    member: &mut ShapeTreeChild,
    transform: &CT_Transform2D,
) -> std::result::Result<(), String> {
    let child_extent = transform
        .child_extent
        .ok_or_else(|| "missing group child extent".to_owned())?;
    if child_extent.cx.0 <= 0
        || child_extent.cy.0 <= 0
        || transform.offset.is_none()
        || transform.extent.is_none()
        || transform.child_offset.is_none()
    {
        return Err("singular or incomplete group transform".to_owned());
    }
    let map = members_to_parent(Some(transform));
    if map == Affine::IDENTITY {
        return Ok(());
    }
    let kind = shape_kind(member);
    let own = shape_transform(member)
        .cloned()
        .ok_or_else(|| "the member has no position and size of its own".to_owned())?;
    let (Some(_offset), Some(extent)) = (own.offset, own.extent) else {
        return Err("the member has no position and size of its own".to_owned());
    };
    let combined = box_to_parent(&own)
        .ok_or_else(|| "missing transform".to_owned())?
        .then(map);
    let [a, b, c, d, _, _] = combined.0;
    let sx = a.hypot(b);
    let sy = c.hypot(d);
    if !combined.0.into_iter().all(f64::is_finite)
        || sx <= f64::EPSILON
        || sy <= f64::EPSILON
        || ((a * c + b * d) / (sx * sy)).abs() > 1e-9
    {
        return Err("singular or skew group composition".to_owned());
    }
    if kind == ShapeKind::GraphicFrame
        && ((a / sx - 1.0).abs() > 1e-9
            || (d / sy - 1.0).abs() > 1e-9
            || b.abs() > 1e-9
            || c.abs() > 1e-9)
    {
        return Err("graphic frame cannot represent rotated or reflected composition".to_owned());
    }
    let width = extent.cx.0 as f64 * sx;
    let height = extent.cy.0 as f64 * sy;
    let center = combined.apply((extent.cx.0 as f64 / 2.0, extent.cy.0 as f64 / 2.0));
    let mut placed = own.clone();
    placed.offset = Some(CT_Point2D {
        x: checked_geometry_emu(center.0 - width / 2.0).map_err(|e| e.to_string())?,
        y: checked_geometry_emu(center.1 - height / 2.0).map_err(|e| e.to_string())?,
    });
    placed.extent = Some(CT_PositiveSize2D {
        cx: checked_geometry_emu(width).map_err(|e| e.to_string())?,
        cy: checked_geometry_emu(height).map_err(|e| e.to_string())?,
    });
    if kind != ShapeKind::GraphicFrame {
        placed.rotation = Angle(
            (b.atan2(a).to_degrees().rem_euclid(360.0) * 60_000.0)
                .round()
                .rem_euclid(21_600_000.0) as i32,
        );
        placed.flip_horizontal = false;
        placed.flip_vertical = a * d - b * c < 0.0;
    }
    let rebuilt =
        box_to_parent(&placed).ok_or_else(|| "invalid decomposed transform".to_owned())?;
    for (u, v) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        let expected = combined.apply((u * extent.cx.0 as f64, v * extent.cy.0 as f64));
        let actual = rebuilt.apply((
            u * placed.extent.unwrap().cx.0 as f64,
            v * placed.extent.unwrap().cy.0 as f64,
        ));
        if (expected.0 - actual.0).hypot(expected.1 - actual.1) > 2.0 {
            return Err("rounded transform cannot preserve four corners".to_owned());
        }
    }
    *shape_mut(member)
        .transform_mut("ungroup shapes")
        .map_err(|e| e.to_string())? = placed;
    Ok(())
}

/// Returns the z-order path of the only shape on a slide with `shape_id`.
pub(super) fn path_of_id(children: &[ShapeTreeChild], shape_id: u32) -> Option<Vec<usize>> {
    for (index, child) in children.iter().enumerate() {
        if child.non_visual_id() == Some(shape_id) {
            return Some(vec![index]);
        }
        if let ShapeTreeChild::GroupShape(group) = child
            && let Some(mut path) = path_of_id(&group.children, shape_id)
        {
            path.insert(0, index);
            return Some(path);
        }
    }
    None
}

pub(super) fn tree_has_connector(children: &[ShapeTreeChild]) -> bool {
    children.iter().any(|child| match child {
        ShapeTreeChild::Connector(_) => true,
        ShapeTreeChild::GroupShape(group) => tree_has_connector(&group.children),
        _ => false,
    })
}

/// Rewrites a connector's transform so that it runs from `begin` to
/// `finish`, in its parent's coordinates, without rotation.
fn place_connector_ends(
    connector: &mut oxoffice_pptx_oxml::connector::CT_ConnectionShape,
    begin: (f64, f64),
    finish: (f64, f64),
) -> Result<()> {
    let offset = CT_Point2D {
        x: checked_geometry_emu(begin.0.min(finish.0))?,
        y: checked_geometry_emu(begin.1.min(finish.1))?,
    };
    let extent = CT_PositiveSize2D {
        cx: checked_geometry_emu((begin.0 - finish.0).abs())?,
        cy: checked_geometry_emu((begin.1 - finish.1).abs())?,
    };
    let transform = connector
        .shape_properties
        .transform
        .get_or_insert_with(CT_Transform2D::default);
    transform.offset = Some(offset);
    transform.extent = Some(extent);
    transform.rotation = Angle(0);
    transform.flip_horizontal = begin.0 > finish.0;
    transform.flip_vertical = begin.1 > finish.1;
    Ok(())
}

impl ShapeMut<'_> {
    /// Moves one end of a connector to `(x, y)` in its parent's
    /// coordinates, as python-pptx `begin_x`, `begin_y`, `end_x`, and
    /// `end_y` assignments do, and releases that end's glue, so PowerPoint
    /// does not route it back to the shape it was glued to. The other end
    /// stays where it is, and the connector's rotation resets to none.
    /// Other shapes are rejected.
    pub fn set_connector_endpoint(&mut self, end: ConnectorEnd, x: Emu, y: Emu) -> Result<()> {
        self.apply_geometry(GeometryEdit::Endpoint(end, x, y))
    }
    fn set_connector_endpoint_leaf(&mut self, end: ConnectorEnd, x: Emu, y: Emu) -> Result<()> {
        const OPERATION: &str = "set connector endpoint";
        let kind = self.kind();
        let ShapeTreeChild::Connector(connector) = self.leaf_mut() else {
            return Err(Error::UnsupportedShapeMutation {
                operation: OPERATION,
                shape_kind: kind,
            });
        };
        let point = (x.0 as f64, y.0 as f64);
        let current = connector
            .shape_properties
            .transform
            .as_ref()
            .and_then(connector_endpoints)
            .unwrap_or([point, point]);
        match end {
            ConnectorEnd::Begin => {
                place_connector_ends(connector, point, current[1])?;
                connector.start_connection = None;
            }
            ConnectorEnd::End => {
                place_connector_ends(connector, current[0], point)?;
                connector.end_connection = None;
            }
        }
        Ok(())
    }
}

/// The begin and end points of a connector in its parent's coordinates.
pub(crate) fn connector_endpoints(transform: &CT_Transform2D) -> Option<[(f64, f64); 2]> {
    let map = box_to_parent(transform)?;
    let extent = transform.extent?;
    Some([
        map.apply((0.0, 0.0)),
        map.apply((extent.cx.0 as f64, extent.cy.0 as f64)),
    ])
}

impl Presentation {
    /// Glues one end of the connector at `connector_path` to connection
    /// site `site` of the shape whose `p:cNvPr/@id` is `shape_id`, as
    /// python-pptx `begin_connect` and `end_connect` do.
    ///
    /// `connector_path` holds one z-order index per nesting level, as for
    /// [`Self::effective_geometry`]. The end is recorded in `a:stCxn` or
    /// `a:endCxn` and moves to the site: the preset geometry's own
    /// connection sites with its adjustments, flips, and rotation, with no guessed cardinal-site fallback. The other end stays where it
    /// is, and the connector's rotation resets to none. A site outside the
    /// shape's sites, a missing or repeated shape id, and a path that names
    /// no connector are rejected without change.
    fn connect_connector_in_candidate(
        &mut self,
        owner: ShapeOwner,
        connector_path: &[usize],
        end: ConnectorEnd,
        shape_id: u32,
        site: u32,
    ) -> Result<()> {
        const OPERATION: &str = "connect connector";
        self.owner_data(owner)?;
        let children = &self.owner_data(owner)?.shape_tree.children;
        let connector = child_at(children, connector_path).ok_or_else(|| {
            invalid_shape_mutation(
                OPERATION,
                format!("owner {owner:?} has no shape at path {connector_path:?}"),
            )
        })?;
        let ShapeTreeChild::Connector(connector) = connector else {
            return Err(Error::UnsupportedShapeMutation {
                operation: OPERATION,
                shape_kind: shape_kind(connector),
            });
        };
        if crate::shape_id_count(children, shape_id) != 1 {
            return Err(invalid_shape_mutation(
                OPERATION,
                format!("shape id {shape_id} is missing or not unique on the slide"),
            ));
        }
        let target_path = path_of_id(children, shape_id).ok_or_else(|| {
            invalid_shape_mutation(
                OPERATION,
                format!("shape id {shape_id} is inside an mc:AlternateContent fallback"),
            )
        })?;
        if target_path == connector_path {
            return Err(invalid_shape_mutation(
                OPERATION,
                "a connector cannot connect to itself",
            ));
        }
        let target = child_at(children, &target_path).expect("the path was found");
        let target_transform = match shape_transform(target) {
            Some(transform) if transform.offset.is_some() && transform.extent.is_some() => {
                transform.clone()
            }
            _ => match owner {
                ShapeOwner::Slide(slide_index) => {
                    self.inherited_target_transform(slide_index, target, OPERATION)?
                }
                _ => self.part_inherited_target_transform(owner, target, OPERATION)?,
            },
        };
        let extent = target_transform
            .extent
            .expect("the transform has an extent");
        let size = (extent.cx.0 as f64, extent.cy.0 as f64);
        let sites = shape_properties(target)
            .and_then(|p| p.preset_geometry.as_ref())
            .ok_or_else(|| invalid_shape_mutation(OPERATION, "no modeled preset sites"))?
            .connection_sites(size)
            .map_err(|e| invalid_shape_mutation(OPERATION, e.to_string()))?
            .filter(|sites| !sites.is_empty())
            .ok_or_else(|| invalid_shape_mutation(OPERATION, "no modeled sites"))?;
        let local = *sites.get(site as usize).ok_or_else(|| {
            invalid_shape_mutation(
                OPERATION,
                format!(
                    "shape id {shape_id} has {} connection sites, 0 to {}, not {site}",
                    sites.len(),
                    sites.len().saturating_sub(1)
                ),
            )
        })?;
        let on_slide = members_to_slide(children, &target_path[..target_path.len() - 1])
            .zip(box_to_parent(&target_transform))
            .map(|(parent, own)| own.then(parent).apply(local))
            .ok_or_else(|| invalid_shape_mutation(OPERATION, "the shape has no position"))?;
        let point = members_to_slide(children, &connector_path[..connector_path.len() - 1])
            .and_then(Affine::inverse)
            .map(|map| map.apply(on_slide))
            .ok_or_else(|| {
                invalid_shape_mutation(
                    OPERATION,
                    "the connector's group has no size, so no point maps into it",
                )
            })?;
        let current = connector
            .shape_properties
            .transform
            .as_ref()
            .and_then(connector_endpoints)
            .unwrap_or([point, point]);
        let [begin, finish] = match end {
            ConnectorEnd::Begin => [point, current[1]],
            ConnectorEnd::End => [current[0], point],
        };

        let children = &mut self.owner_data_mut(owner)?.shape_tree.children;
        let Some(ShapeTreeChild::Connector(connector)) = child_at_mut(children, connector_path)
        else {
            unreachable!("the connector was found above");
        };
        place_connector_ends(connector, begin, finish)?;
        let connection = match end {
            ConnectorEnd::Begin => &mut connector.start_connection,
            ConnectorEnd::End => &mut connector.end_connection,
        };
        if let Some(connection) = connection {
            connection.id = shape_id;
            connection.idx = site;
        } else {
            *connection = Some(oxoffice_pptx_oxml::connector::CT_Connection::new(
                shape_id, site,
            ));
        }
        Ok(())
    }

    fn inherited_target_transform(
        &self,
        slide_index: usize,
        target: &ShapeTreeChild,
        operation: &'static str,
    ) -> Result<CT_Transform2D> {
        let inherited = match shape_placeholder(target) {
            Some(placeholder) => self.inherited_transform(slide_index, placeholder)?,
            None => None,
        };
        inherited
            .filter(|transform| transform.offset.is_some() && transform.extent.is_some())
            .ok_or_else(|| invalid_shape_mutation(operation, "the shape has no position and size"))
    }
}

impl Presentation {
    pub fn connect_connector(
        &mut self,
        slide: usize,
        path: &[usize],
        end: ConnectorEnd,
        target: u32,
        site: u32,
    ) -> Result<()> {
        self.connect_connector_at(ShapeOwner::Slide(slide), path, end, target, site)
    }

    /// Stages a connector attachment in its actual owning part.
    pub fn connect_connector_at(
        &mut self,
        owner: ShapeOwner,
        path: &[usize],
        end: ConnectorEnd,
        target: u32,
        site: u32,
    ) -> Result<()> {
        self.owner_data(owner)?;
        let mut candidate = self.clone();
        candidate.connect_connector_in_candidate(owner, path, end, target, site)?;
        candidate.validate_owned_part(owner)?;
        *self = candidate;
        Ok(())
    }
    pub(super) fn refresh_connector_attachments(
        &mut self,
        owner: ShapeOwner,
        previous_children: &[ShapeTreeChild],
        removed: &HashSet<u32>,
    ) -> Result<()> {
        self.owner_data(owner)?;
        let mut attachments = Vec::new();
        collect_connector_attachments(
            &self.owner_data(owner)?.shape_tree.children,
            &mut Vec::new(),
            &mut attachments,
        );
        detach_connectors(
            &mut self.owner_data_mut(owner)?.shape_tree.children,
            removed,
        );
        for (path, end, id, site) in attachments {
            if removed.contains(&id) {
                continue;
            }
            let before = geometry_target_signature(previous_children, id);
            // Never adopt a formerly dangling reference by incidental ID allocation.
            if before.is_empty() {
                continue;
            }
            let after = geometry_target_signature(&self.owner_data(owner)?.shape_tree.children, id);
            let connector_id = child_at(&self.owner_data(owner)?.shape_tree.children, &path)
                .and_then(ShapeTreeChild::non_visual_id);
            let before_children = previous_children;
            let after_children = &self.owner_data(owner)?.shape_tree.children;
            let connector_changed = connector_id.is_some_and(|connector_id| {
                geometry_target_signature(before_children, connector_id)
                    != geometry_target_signature(after_children, connector_id)
            });
            if before != after || connector_changed {
                self.connect_connector_in_candidate(owner, &path, end, id, site)?;
            }
        }
        Ok(())
    }
    /// Path-aware mutation stages attached endpoints with the actual target edit.
    pub fn set_shape_position(
        &mut self,
        slide: usize,
        path: &[usize],
        left: Emu,
        top: Emu,
    ) -> Result<()> {
        self.edit_shape_geometry(
            ShapeOwner::Slide(slide),
            path,
            GeometryEdit::Position(left, top),
        )
    }
    pub fn set_shape_size(
        &mut self,
        slide: usize,
        path: &[usize],
        width: Emu,
        height: Emu,
    ) -> Result<()> {
        if width.0 < 0 || height.0 < 0 {
            return Err(invalid_shape_mutation("set size", "negative extent"));
        }
        self.edit_shape_geometry(
            ShapeOwner::Slide(slide),
            path,
            GeometryEdit::Size(width, height),
        )
    }
    pub fn set_shape_rotation(
        &mut self,
        slide: usize,
        path: &[usize],
        rotation: Angle,
    ) -> Result<()> {
        self.edit_shape_geometry(
            ShapeOwner::Slide(slide),
            path,
            GeometryEdit::Rotation(rotation),
        )
    }
    pub fn set_shape_flips(
        &mut self,
        slide: usize,
        path: &[usize],
        horizontal: bool,
        vertical: bool,
    ) -> Result<()> {
        self.edit_shape_geometry(
            ShapeOwner::Slide(slide),
            path,
            GeometryEdit::Flips(horizontal, vertical),
        )
    }
    pub(super) fn edit_shape_geometry(
        &mut self,
        owner: ShapeOwner,
        path: &[usize],
        edit: GeometryEdit,
    ) -> Result<()> {
        self.owner_data(owner)?;
        if let GeometryEdit::Size(width, height) = &edit
            && (width.0 < 0 || height.0 < 0)
        {
            return Err(invalid_shape_mutation("set size", "negative extent"));
        }
        let has_connector = tree_has_connector(&self.owner_data(owner)?.shape_tree.children);
        if has_connector {
            let previous = self.owner_data(owner)?.shape_tree.children.clone();
            let mut candidate = self.clone();
            candidate.materialize_owner_geometry(owner, path)?;
            let child = child_at_mut(
                &mut candidate.owner_data_mut(owner)?.shape_tree.children,
                path,
            )
            .ok_or_else(|| invalid_shape_mutation("shape geometry", "missing physical owner"))?;
            apply_leaf_geometry(child, edit)?;
            candidate.refresh_connector_attachments(owner, &previous, &HashSet::new())?;
            candidate.validate_owned_part(owner)?;
            *self = candidate;
            return Ok(());
        }
        let snapshot = child_at(&self.owner_data(owner)?.shape_tree.children, path)
            .ok_or_else(|| invalid_shape_mutation("shape geometry", "missing physical owner"))?
            .clone();
        let part_name = self.owner_part_name(owner)?.to_owned();
        let edited = (|| {
            self.materialize_owner_geometry(owner, path)?;
            let child = child_at_mut(&mut self.owner_data_mut(owner)?.shape_tree.children, path)
                .ok_or_else(|| {
                    invalid_shape_mutation("shape geometry", "missing physical owner")
                })?;
            apply_leaf_geometry(child, edit)?;
            let child =
                child_at(&self.owner_data(owner)?.shape_tree.children, path).ok_or_else(|| {
                    invalid_shape_mutation("shape geometry", "missing physical owner")
                })?;
            validate_shape_child(child, &part_name)
        })();
        if let Err(error) = edited {
            if let Some(child) =
                child_at_mut(&mut self.owner_data_mut(owner)?.shape_tree.children, path)
            {
                *child = snapshot;
            }
            return Err(error);
        }
        Ok(())
    }
}
pub(super) enum GeometryEdit {
    Position(Emu, Emu),
    Size(Emu, Emu),
    Rotation(Angle),
    Flips(bool, bool),
    Endpoint(ConnectorEnd, Emu, Emu),
    Adjustment(String, f64),
    Preset(String),
    TableColumn(usize, Emu),
    TableRow(usize, Emu),
    TableInsertRow(usize),
    TableRemoveRow(usize),
    TableInsertColumn(usize),
    TableRemoveColumn(usize),
}
fn collect_connector_attachments(
    children: &[ShapeTreeChild],
    path: &mut Vec<usize>,
    out: &mut Vec<(Vec<usize>, ConnectorEnd, u32, u32)>,
) {
    for (index, child) in children.iter().enumerate() {
        path.push(index);
        match child {
            ShapeTreeChild::Connector(connector) => {
                if let Some(c) = &connector.start_connection {
                    out.push((path.clone(), ConnectorEnd::Begin, c.id, c.idx));
                }
                if let Some(c) = &connector.end_connection {
                    out.push((path.clone(), ConnectorEnd::End, c.id, c.idx));
                }
            }
            ShapeTreeChild::GroupShape(group) => {
                collect_connector_attachments(&group.children, path, out)
            }
            _ => (),
        }
        path.pop();
    }
}
impl ShapesMut<'_> {
    pub fn group(&mut self, indices: &[usize]) -> Result<ShapeMut<'_>> {
        let mut candidate = self.presentation.clone();
        let id = candidate
            .shapes_mut_at(self.owner, &self.group)
            .map_err(|_| invalid_shape_mutation("group shapes", "missing parent"))?
            .group_in_candidate(indices)?
            .leaf()
            .non_visual_id()
            .ok_or_else(|| invalid_shape_mutation("group shapes", "missing new group ID"))?;
        candidate.refresh_connector_attachments(
            self.owner,
            &self
                .presentation
                .owner_data(self.owner)?
                .shape_tree
                .children,
            &HashSet::new(),
        )?;
        let path = path_of_id(&candidate.owner_data(self.owner)?.shape_tree.children, id)
            .ok_or_else(|| invalid_shape_mutation("group shapes", "new group missing"))?;
        candidate.validate_owned_part(self.owner)?;
        *self.presentation = candidate;
        Ok(owned_shape_mut(self.presentation, self.owner, path))
    }
    pub fn ungroup(&mut self, index: usize) -> Result<std::ops::Range<usize>> {
        let mut candidate = self.presentation.clone();
        let mut path = self.group.clone();
        path.push(index);
        let id = child_at(
            &candidate.owner_data(self.owner)?.shape_tree.children,
            &path,
        )
        .and_then(|c| c.non_visual_id())
        .ok_or_else(|| invalid_shape_mutation("ungroup shapes", "missing group identity"))?;
        let range = candidate
            .shapes_mut_at(self.owner, &self.group)
            .map_err(|_| invalid_shape_mutation("ungroup shapes", "missing parent"))?
            .ungroup_in_candidate(index)?;
        candidate.refresh_connector_attachments(
            self.owner,
            &self
                .presentation
                .owner_data(self.owner)?
                .shape_tree
                .children,
            &HashSet::from([id]),
        )?;
        candidate.validate_owned_part(self.owner)?;
        *self.presentation = candidate;
        Ok(range)
    }
    pub fn align(
        &mut self,
        indices: &[usize],
        alignment: ShapeAlignment,
        reference: ArrangeReference,
    ) -> Result<()> {
        let mut candidate = self.presentation.clone();
        candidate
            .shapes_mut_at(self.owner, &self.group)
            .map_err(|_| invalid_shape_mutation("align shapes", "missing parent"))?
            .align_in_candidate(indices, alignment, reference)?;
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
        *self.presentation = candidate;
        Ok(())
    }
    pub fn distribute(
        &mut self,
        indices: &[usize],
        direction: DistributeDirection,
        reference: ArrangeReference,
    ) -> Result<()> {
        let mut candidate = self.presentation.clone();
        candidate
            .shapes_mut_at(self.owner, &self.group)
            .map_err(|_| invalid_shape_mutation("distribute shapes", "missing parent"))?
            .distribute_in_candidate(indices, direction, reference)?;
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
        *self.presentation = candidate;
        Ok(())
    }
}

pub(super) fn apply_leaf_geometry(child: &mut ShapeTreeChild, edit: GeometryEdit) -> Result<()> {
    let mut shape = shape_mut(child);
    match edit {
        GeometryEdit::Endpoint(end, x, y) => shape.set_connector_endpoint_leaf(end, x, y),
        GeometryEdit::Position(x, y) => shape.set_position_leaf(x, y),
        GeometryEdit::Size(w, h) => shape.set_size_leaf(w, h),
        GeometryEdit::Rotation(r) => shape.set_rotation_leaf(r),
        GeometryEdit::Flips(h, v) => {
            let t = shape.transform_mut("set flips")?;
            t.flip_horizontal = h;
            t.flip_vertical = v;
            Ok(())
        }
        GeometryEdit::Adjustment(name, value) => shape.set_adjust_value_leaf(&name, value),
        GeometryEdit::Preset(name) => shape.set_auto_shape_type_leaf(&name),
        GeometryEdit::TableColumn(index, width) => shape
            .into_table_mut()
            .ok_or_else(|| invalid_shape_mutation("set column width", "not a table"))?
            .set_column_width_leaf(index, width),
        GeometryEdit::TableRow(index, height) => shape
            .into_table_mut()
            .ok_or_else(|| invalid_shape_mutation("set row height", "not a table"))?
            .set_row_height_leaf(index, height),
        GeometryEdit::TableInsertRow(index) => shape
            .into_table_mut()
            .ok_or_else(|| invalid_shape_mutation("insert row", "not a table"))?
            .insert_row_leaf(index),
        GeometryEdit::TableRemoveRow(index) => shape
            .into_table_mut()
            .ok_or_else(|| invalid_shape_mutation("remove row", "not a table"))?
            .remove_row_leaf(index),
        GeometryEdit::TableInsertColumn(index) => shape
            .into_table_mut()
            .ok_or_else(|| invalid_shape_mutation("insert column", "not a table"))?
            .insert_column_leaf(index),
        GeometryEdit::TableRemoveColumn(index) => shape
            .into_table_mut()
            .ok_or_else(|| invalid_shape_mutation("remove column", "not a table"))?
            .remove_column_leaf(index),
    }
}

#[derive(PartialEq)]
struct TargetGeometry {
    transform: Option<CT_Transform2D>,
    preset: Option<CT_PresetGeometry2D>,
    parents: Vec<CT_Transform2D>,
}
fn geometry_target_signature(children: &[ShapeTreeChild], id: u32) -> Vec<TargetGeometry> {
    fn visit(
        children: &[ShapeTreeChild],
        id: u32,
        parents: &mut Vec<CT_Transform2D>,
        out: &mut Vec<TargetGeometry>,
    ) {
        for child in children {
            if child.non_visual_id() == Some(id) {
                out.push(TargetGeometry {
                    transform: shape_transform(child).cloned(),
                    preset: shape_properties(child).and_then(|p| p.preset_geometry.clone()),
                    parents: parents.clone(),
                });
            }
            if let ShapeTreeChild::GroupShape(group) = child {
                parents.push(group.group_transform().cloned().unwrap_or_default());
                visit(&group.children, id, parents, out);
                parents.pop();
            }
        }
    }
    let mut out = Vec::new();
    visit(children, id, &mut Vec::new(), &mut out);
    out
}

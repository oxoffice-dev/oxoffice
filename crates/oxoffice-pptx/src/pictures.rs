//! Pictures and fills: adding pictures, filling picture placeholders, shape and picture fills, and reading and replacing a picture's image.

use super::*;

/// The embedded image part shown by one picture shape.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PictureImage<'a> {
    pub part_name: String,
    pub content_type: String,
    pub bytes: &'a [u8],
}

impl Presentation {
    /// Appends a relationship-backed picture at the top of one slide's z-order.
    #[allow(clippy::too_many_arguments)]
    pub fn add_picture(
        &mut self,
        slide_index: usize,
        image_data: &[u8],
        image_filename: &str,
        left: Emu,
        top: Emu,
        width: Option<Emu>,
        height: Option<Emu>,
    ) -> Result<ShapeRef<'_>> {
        self.append_picture(
            ShapeOwner::Slide(slide_index),
            &[],
            image_data,
            image_filename,
            left,
            top,
            width,
            height,
        )
        .map(|picture| shape_ref(picture))
    }

    /// Stages the image part and slide relationship of a new picture, then
    /// appends it to the slide's own shapes or to the group that `group`
    /// indexes, which callers have checked.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn append_picture(
        &mut self,
        owner: ShapeOwner,
        group: &[usize],
        image_data: &[u8],
        image_filename: &str,
        left: Emu,
        top: Emu,
        width: Option<Emu>,
        height: Option<Emu>,
    ) -> Result<&mut ShapeTreeChild> {
        let slide_part = self.owner_part_name(owner)?.to_owned();
        let (width, height) = picture_dimensions(image_data, image_filename, width, height)?;
        let id = ShapeIdAllocator::scan(&self.owner_data(owner)?.shape_tree).allocate();

        let mut package = self.package.clone();
        let mut media_store = self.media_store.clone();
        let media_part = media_store.insert(&mut package, image_data, image_filename);
        let mut relationships = package
            .get_part_rels(&slide_part)
            .cloned()
            .unwrap_or_default();
        let relationship_id = relationships
            .items
            .iter()
            .find(|relationship| {
                relationship.rel_type == rel_types::IMAGE
                    && !relationship_is_external(relationship)
                    && OpcPackage::resolve_rel_target(&slide_part, &relationship.target)
                        == media_part
            })
            .map(|relationship| relationship.id.clone())
            .unwrap_or_else(|| {
                relationships.add(
                    rel_types::IMAGE,
                    &relative_part_target(&slide_part, &media_part),
                )
            });
        let picture = CT_Picture::new(
            id,
            &format!("Picture {id}"),
            &relationship_id,
            positioned_transform(left, top, width, height),
        )
        .map_err(|error| invalid_shape_construction("add picture", error))?;

        package.set_part_rels(&slide_part, relationships);
        self.package = package;
        self.media_store = media_store;
        let tree = &mut self.owner_data_mut(owner)?.shape_tree;
        Ok(append_member(tree, group, ShapeTreeChild::Picture(picture)))
    }

    /// Fills one selected picture placeholder in place, preserving its owner order.
    /// An unsupported metadata conversion leaves both package and tree unchanged.
    pub fn fill_picture_placeholder(
        &mut self,
        slide_index: usize,
        shape_path: &[usize],
        image_data: &[u8],
        image_filename: &str,
    ) -> Result<()> {
        const OPERATION: &str = "fill picture placeholder";
        self.require_slide_index(slide_index)?;
        let child = child_at(
            &self.slides[slide_index]
                .slide
                .common_slide_data
                .shape_tree
                .children,
            shape_path,
        )
        .ok_or_else(|| invalid_shape_mutation(OPERATION, "placeholder owner is absent"))?;
        let ShapeTreeChild::Shape(source) = child else {
            return Err(invalid_shape_mutation(
                OPERATION,
                "owner is not an unfilled picture placeholder",
            ));
        };
        if source.placeholder.as_ref().map(|ph| ph.effective_type()) != Some(PhType::Picture) {
            return Err(invalid_shape_mutation(
                OPERATION,
                "owner is not a picture placeholder",
            ));
        }
        let mut candidate = self.clone();
        candidate.materialize_geometry(slide_index, shape_path)?;
        let transform = shape_transform(
            candidate
                .shape_at(slide_index, shape_path, OPERATION)?
                .child,
        )
        .cloned()
        .ok_or_else(|| {
            invalid_shape_mutation(OPERATION, "placeholder has no inherited rectangle")
        })?;
        let extent = transform.extent.ok_or_else(|| {
            invalid_shape_mutation(OPERATION, "placeholder has no inherited rectangle")
        })?;
        if extent.cx.0 <= 0 || extent.cy.0 <= 0 {
            return Err(invalid_shape_mutation(
                OPERATION,
                "placeholder rectangle must be positive",
            ));
        }
        if ImageFormat::sniff(image_data).is_none() {
            return Err(Error::UnsupportedPicture {
                filename: image_filename.to_owned(),
            });
        }
        let info = probe(image_data).ok_or_else(|| Error::UnavailablePictureDimensions {
            filename: image_filename.to_owned(),
        })?;
        if info.width_px == 0 || info.height_px == 0 {
            return Err(Error::UnavailablePictureDimensions {
                filename: image_filename.to_owned(),
            });
        }
        let slide_part = candidate.slides[slide_index].part_name.clone();
        let media_part =
            candidate
                .media_store
                .insert(&mut candidate.package, image_data, image_filename);
        let mut rels = candidate
            .package
            .get_part_rels(&slide_part)
            .cloned()
            .unwrap_or_default();
        let relationship_id = rels
            .items
            .iter()
            .find(|rel| {
                rel.rel_type == rel_types::IMAGE
                    && !relationship_is_external(rel)
                    && OpcPackage::resolve_rel_target(&slide_part, &rel.target) == media_part
            })
            .map(|rel| rel.id.clone())
            .unwrap_or_else(|| {
                rels.add(
                    rel_types::IMAGE,
                    &relative_part_target(&slide_part, &media_part),
                )
            });
        let mut picture = source
            .fill_picture(&relationship_id, transform)
            .map_err(|error| invalid_shape_mutation(OPERATION, error.to_string()))?;
        // Center-crop by decoded pixels, independent of the image's DPI metadata.
        let image_aspect = f64::from(info.width_px) / f64::from(info.height_px);
        let box_aspect = extent.cx.0 as f64 / extent.cy.0 as f64;
        let horizontal = image_aspect > box_aspect;
        let edge = if horizontal {
            (1.0 - box_aspect / image_aspect) * 50_000.0
        } else {
            (1.0 - image_aspect / box_aspect) * 50_000.0
        };
        let edge = Percent1000(edge.round() as i32);
        let mut rect = RelativeRect::default();
        if horizontal {
            rect.left = Some(edge);
            rect.right = Some(edge);
        } else {
            rect.top = Some(edge);
            rect.bottom = Some(edge);
        }
        if edge.0 != 0 {
            picture
                .blip_fill
                .as_mut()
                .ok_or_else(|| invalid_shape_mutation(OPERATION, "picture has no image fill"))?
                .source_rect = Some(rect);
        }
        candidate.package.set_part_rels(&slide_part, rels);
        *child_at_mut(
            &mut Arc::make_mut(&mut candidate.slides[slide_index])
                .slide
                .common_slide_data
                .shape_tree
                .children,
            shape_path,
        )
        .ok_or_else(|| invalid_shape_mutation(OPERATION, "placeholder owner changed"))? =
            ShapeTreeChild::Picture(picture);
        candidate.saved_package_read_back()?;
        *self = candidate;
        Ok(())
    }

    /// Replaces one owned fill in a staged candidate and prunes only image
    /// relationships that no qualified source owner still references.
    pub fn set_fill_at(&mut self, owner: &FillOwnerLocation, fill: Fill) -> Result<()> {
        if self.local_shape_fill(owner, &fill)? {
            return self.assign_local_shape_fill(owner, fill);
        }
        let mut candidate = self.clone();
        candidate.assign_fill_at(owner, fill)?;
        if candidate.owner_data(owner.owner())? == self.owner_data(owner.owner())? {
            return Ok(());
        }
        let previous = self.owner_relationship_ids(owner.owner())?;
        candidate.release_unused_owner_images(owner.owner(), &previous)?;
        candidate.commit_owned_parts(None)?;
        *self = candidate;
        Ok(())
    }

    /// A solid, gradient, pattern or no-fill on one shape does not change
    /// image relationships. Assign it on that shape and validate only that
    /// child. Blip fills and every other owner keep the package transaction.
    fn local_shape_fill(&self, location: &FillOwnerLocation, fill: &Fill) -> Result<bool> {
        if matches!(fill, Fill::Blip(_)) {
            return Ok(false);
        }
        let (owner, path) = match location {
            FillOwnerLocation::Shape {
                slide_index,
                shape_path,
            } => (ShapeOwner::Slide(*slide_index), shape_path.as_slice()),
            FillOwnerLocation::OwnedShape { owner, shape_path } => (*owner, shape_path.as_slice()),
            _ => return Ok(false),
        };
        let child =
            child_at(&self.owner_data(owner)?.shape_tree.children, path).ok_or_else(|| {
                invalid_shape_mutation("set owned fill", "fill owner is absent or unsupported")
            })?;
        let current = shape_properties(child).and_then(|properties| properties.fill.as_ref());
        Ok(!matches!(current, Some(Fill::Blip(_))))
    }

    fn assign_local_shape_fill(&mut self, location: &FillOwnerLocation, fill: Fill) -> Result<()> {
        let owner = location.owner();
        let path = match location {
            FillOwnerLocation::Shape { shape_path, .. }
            | FillOwnerLocation::OwnedShape { shape_path, .. } => shape_path.clone(),
            _ => unreachable!("local shape fill rejected every other owner"),
        };
        let snapshot = child_at(&self.owner_data(owner)?.shape_tree.children, &path)
            .ok_or_else(|| {
                invalid_shape_mutation("set owned fill", "fill owner is absent or unsupported")
            })?
            .clone();
        let assigned = self.assign_fill_at(location, fill);
        if let Err(error) = assigned {
            if let Some(child) =
                child_at_mut(&mut self.owner_data_mut(owner)?.shape_tree.children, &path)
            {
                *child = snapshot;
            }
            return Err(error);
        }
        let part_name = self.owner_part_name(owner)?.to_owned();
        let validated = child_at(&self.owner_data(owner)?.shape_tree.children, &path)
            .ok_or_else(|| {
                invalid_shape_mutation("set owned fill", "fill owner is absent or unsupported")
            })
            .and_then(|child| validate_shape_child(child, &part_name));
        if let Err(error) = validated {
            if let Some(child) =
                child_at_mut(&mut self.owner_data_mut(owner)?.shape_tree.children, &path)
            {
                *child = snapshot;
            }
            return Err(error);
        }
        Ok(())
    }

    /// Validates an image, then stages allocation, owner fill and reference
    /// pruning before publishing any package change.
    pub fn set_picture_fill(
        &mut self,
        owner: &FillOwnerLocation,
        image_data: &[u8],
        filename: &str,
    ) -> Result<()> {
        self.owner_data(owner.owner())?;
        picture_dimensions(image_data, filename, None, None)?;
        let mut candidate = self.clone();
        let slide_part = candidate.owner_part_name(owner.owner())?.to_owned();
        let media_part = candidate
            .media_store
            .insert(&mut candidate.package, image_data, filename);
        let mut relationships = candidate
            .package
            .get_part_rels(&slide_part)
            .cloned()
            .unwrap_or_default();
        let id = relationships
            .items
            .iter()
            .find(|relationship| {
                relationship.rel_type == rel_types::IMAGE
                    && !relationship_is_external(relationship)
                    && OpcPackage::resolve_rel_target(&slide_part, &relationship.target)
                        == media_part
            })
            .map(|relationship| relationship.id.clone())
            .unwrap_or_else(|| {
                relationships.add(
                    rel_types::IMAGE,
                    &relative_part_target(&slide_part, &media_part),
                )
            });
        candidate.package.set_part_rels(&slide_part, relationships);
        let source = format!(
            r#"<a:blipFill xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" rotWithShape="1"><a:blip r:embed="{}"/><a:stretch><a:fillRect/></a:stretch></a:blipFill>"#,
            quick_xml::escape::escape(&id)
        );
        let fill = Fill::from_xml(source.as_bytes())
            .map_err(|error| invalid_shape_mutation("set picture fill", error.to_string()))?;
        candidate.assign_fill_at(owner, fill)?;
        let previous = self.owner_relationship_ids(owner.owner())?;
        candidate.release_unused_owner_images(owner.owner(), &previous)?;
        candidate.commit_owned_parts(None)?;
        *self = candidate;
        Ok(())
    }

    fn assign_fill_at(&mut self, location: &FillOwnerLocation, fill: Fill) -> Result<()> {
        let owner = location.owner();
        let missing =
            || invalid_shape_mutation("set owned fill", "fill owner is absent or unsupported");
        self.owner_data(owner)?;
        if matches!(
            location,
            FillOwnerLocation::SlideBackground { .. } | FillOwnerLocation::OwnedBackground { .. }
        ) {
            let common = self.owner_data_mut(owner)?;
            if let Some(background) = &mut common.background {
                background
                    .replace_fill(fill)
                    .map_err(|error| invalid_shape_mutation("set background", error.to_string()))?;
            } else {
                common.background = Some(
                    oxoffice_pptx_oxml::slide_parts::CT_Background::from_fill(fill).map_err(
                        |error| invalid_shape_mutation("set background", error.to_string()),
                    )?,
                );
            }
            return Ok(());
        }
        let path = match location {
            FillOwnerLocation::Shape { shape_path, .. }
            | FillOwnerLocation::TableCell { shape_path, .. }
            | FillOwnerLocation::OwnedShape { shape_path, .. }
            | FillOwnerLocation::OwnedTableCell { shape_path, .. } => shape_path,
            _ => unreachable!(),
        };
        let mut shape = self.shape_mut_at(owner, path)?;
        match location {
            FillOwnerLocation::Shape { .. } | FillOwnerLocation::OwnedShape { .. } => {
                shape.set_fill(fill)
            }
            FillOwnerLocation::TableCell { row, column, .. }
            | FillOwnerLocation::OwnedTableCell { row, column, .. } => {
                let mut cell = shape
                    .into_table_mut()
                    .and_then(|table| table.into_cell_mut(*row, *column))
                    .ok_or_else(missing)?;
                cell.set_fill(Some(fill));
                Ok(())
            }
            _ => unreachable!(),
        }
    }

    pub(super) fn owner_relationship_ids(&self, owner: ShapeOwner) -> Result<HashSet<String>> {
        let part = self.owner_part_name(owner)?;
        let xml = match owner {
            ShapeOwner::Slide(index) => self.slides[index].slide.to_xml(),
            ShapeOwner::Layout(index) => self.layouts[index].layout.to_xml(),
            ShapeOwner::Master(index) => self.masters[index]
                .master
                .as_ref()
                .map_err(|message| Error::MalformedPart {
                    part_name: part.to_owned(),
                    message: message.clone(),
                })?
                .to_xml(),
        }
        .map_err(|error| Error::MalformedPart {
            part_name: part.to_owned(),
            message: error.to_string(),
        })?;
        relationship_ids(&xml)
            .map(|ids| ids.into_iter().collect())
            .map_err(|error| invalid_shape_mutation("owned image references", error.to_string()))
    }

    pub(super) fn release_unused_owner_images(
        &mut self,
        owner: ShapeOwner,
        previous: &HashSet<String>,
    ) -> Result<()> {
        let part_name = self.owner_part_name(owner)?.to_owned();
        let referenced = self.owner_relationship_ids(owner)?;
        let Some(mut relationships) = self.package.get_part_rels(&part_name).cloned() else {
            return Ok(());
        };
        let mut released = HashSet::new();
        relationships.items.retain(|relationship| {
            let unused = previous.contains(&relationship.id)
                && relationship.rel_type == rel_types::IMAGE
                && !relationship_is_external(relationship)
                && !referenced.contains(&relationship.id);
            if unused {
                released.insert(OpcPackage::resolve_rel_target(
                    &part_name,
                    &relationship.target,
                ));
            }
            !unused
        });
        if !released.is_empty() {
            self.package.set_part_rels(&part_name, relationships);
            prune_unreachable_parts(&mut self.package, &released);
            self.media_store = self.media_store.rescan(&self.package);
        }
        Ok(())
    }

    /// Returns the embedded image one picture shows, found by shape id.
    ///
    /// Pictures inside groups and alternate-content fallbacks are found too.
    /// The content type comes from the package, falling back to the sniffed
    /// image format.
    pub fn picture_image(&self, slide_index: usize, shape_id: u32) -> Result<PictureImage<'_>> {
        self.picture_image_at(ShapeOwner::Slide(slide_index), shape_id)
    }

    /// Reads an image from its actual slide, layout or master relationship scope.
    pub fn picture_image_at(&self, owner: ShapeOwner, shape_id: u32) -> Result<PictureImage<'_>> {
        let data = self.owner_data(owner)?;
        if picture_shape_id_count(&data.shape_tree.children, shape_id) != 1 {
            return Err(invalid_shape_mutation(
                "read picture image",
                format!("shape id {shape_id} is missing or ambiguous"),
            ));
        }
        let source_part = self.owner_part_name(owner)?;
        let picture = find_picture(&data.shape_tree.children, shape_id)
            .ok_or_else(|| not_a_picture("read picture image", shape_id))?;
        let relationship_id = picture_image_relationship(picture, "read picture image", shape_id)?;
        let part_name = require_internal_related_part(
            &self.package,
            source_part,
            relationship_id,
            rel_types::IMAGE,
        )?;
        let bytes = required_part(&self.package, &part_name)?;
        let content_type = self
            .package
            .content_types
            .content_type_for(&part_name)
            .map_or_else(
                || resolve(bytes, &part_name).content_type().to_owned(),
                str::to_owned,
            );
        Ok(PictureImage {
            part_name,
            content_type,
            bytes,
        })
    }

    /// Replaces the image one picture shows while keeping its geometry.
    ///
    /// Only this picture changes. A relationship other pictures share is
    /// split first, a part no other relationship targets is rewritten in
    /// place when its extension fits the new format, and any other part is
    /// left to its remaining users while the picture moves to a new or equal
    /// media part. A picture that carries a second image, such as an SVG
    /// alternate, is rejected because PowerPoint would keep showing it.
    pub fn replace_picture_image(
        &mut self,
        slide_index: usize,
        shape_id: u32,
        image_data: &[u8],
    ) -> Result<()> {
        self.replace_picture_image_at(ShapeOwner::Slide(slide_index), shape_id, image_data)
    }

    /// Replaces an image through its physical part owner with the same copy-on-write rules.
    pub fn replace_picture_image_at(
        &mut self,
        owner: ShapeOwner,
        shape_id: u32,
        image_data: &[u8],
    ) -> Result<()> {
        const OPERATION: &str = "replace picture image";
        let data = self.owner_data(owner)?;
        if picture_shape_id_count(&data.shape_tree.children, shape_id) != 1 {
            return Err(invalid_shape_mutation(
                OPERATION,
                format!("shape id {shape_id} is missing or ambiguous"),
            ));
        }
        let format = ImageFormat::sniff(image_data).ok_or_else(|| Error::InvalidShapeMutation {
            operation: OPERATION,
            message: "replacement bytes are not a supported image".to_owned(),
        })?;
        let mut staged = self.clone();
        let slide_part = staged.owner_part_name(owner)?.to_owned();
        let picture = find_picture(&staged.owner_data(owner)?.shape_tree.children, shape_id)
            .ok_or_else(|| not_a_picture(OPERATION, shape_id))?;
        let relationship_id = picture_image_relationship(picture, OPERATION, shape_id)?.to_owned();
        let old_part = require_internal_related_part(
            &staged.package,
            &slide_part,
            &relationship_id,
            rel_types::IMAGE,
        )?;
        let mut relationships = staged
            .package
            .get_part_rels(&slide_part)
            .cloned()
            .unwrap_or_default();
        let picture_xml = picture
            .to_xml()
            .map_err(|error| invalid_shape_mutation(OPERATION, error.to_string()))?;
        let second_image = relationship_ids(&picture_xml)
            .map_err(|error| invalid_shape_mutation(OPERATION, error.to_string()))?
            .into_iter()
            .filter(|id| *id != relationship_id)
            .any(|id| {
                relationships
                    .get_by_id(&id)
                    .is_some_and(|relationship| relationship.rel_type == rel_types::IMAGE)
            });
        if second_image {
            return Err(invalid_shape_mutation(
                OPERATION,
                format!("picture {shape_id} carries a second image, such as an SVG alternate"),
            ));
        }
        let slide_xml = match owner {
            ShapeOwner::Slide(index) => staged.slides[index].slide.to_xml(),
            ShapeOwner::Layout(index) => staged.layouts[index].layout.to_xml(),
            ShapeOwner::Master(index) => staged.masters[index]
                .master
                .as_ref()
                .expect("checked owner")
                .to_xml(),
        }
        .map_err(|error| invalid_shape_mutation(OPERATION, error.to_string()))?;
        let uses = relationship_ids(&slide_xml)
            .map_err(|error| invalid_shape_mutation(OPERATION, error.to_string()))?
            .into_iter()
            .filter(|id| *id == relationship_id)
            .count();
        let picture = find_picture_mut(
            &mut staged.owner_data_mut(owner)?.shape_tree.children,
            shape_id,
        )
        .ok_or_else(|| {
            invalid_shape_mutation(
                OPERATION,
                "picture in raw alternate content cannot be replaced",
            )
        })?;
        let relationship_id = if uses > 1 {
            let isolated = relationships.add(
                rel_types::IMAGE,
                &relative_part_target(&slide_part, &old_part),
            );
            picture
                .blip_fill
                .as_mut()
                .and_then(|fill| fill.blip.as_mut())
                .expect("picture has an embedded image")
                .embed = Some(isolated.clone());
            isolated
        } else {
            relationship_id
        };
        let part_users = std::iter::once(("/", &staged.package.package_rels))
            .chain(
                staged
                    .package
                    .part_rels
                    .iter()
                    .filter(|(source, _)| **source != slide_part)
                    .map(|(source, items)| (source.as_str(), items)),
            )
            .chain(std::iter::once((slide_part.as_str(), &relationships)))
            .flat_map(|(source, items)| items.items.iter().map(move |item| (source, item)))
            .filter(|(source, relationship)| {
                !relationship_is_external(relationship)
                    && OpcPackage::resolve_rel_target(source, &relationship.target)
                        .eq_ignore_ascii_case(&old_part)
            })
            .count();
        let extension = old_part
            .rsplit_once('.')
            .map_or("", |(_, extension)| extension)
            .to_owned();
        if part_users == 1 && ImageFormat::from_extension(&extension) == Some(format) {
            staged.package.set_part(&old_part, image_data.to_vec());
            register_content_type(
                &mut staged.package,
                &old_part,
                &extension,
                format.content_type(),
            );
        } else {
            let new_part = staged.media_store.insert(
                &mut staged.package,
                image_data,
                &format!("image.{}", format.extension()),
            );
            let relationship = relationships
                .items
                .iter_mut()
                .find(|relationship| relationship.id == relationship_id)
                .expect("picture relationship exists");
            relationship.target = relative_part_target(&slide_part, &new_part);
        }
        staged.package.set_part_rels(&slide_part, relationships);
        prune_unreachable_parts(&mut staged.package, &HashSet::from([old_part]));
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

fn not_a_picture(operation: &'static str, shape_id: u32) -> Error {
    invalid_shape_mutation(operation, format!("shape id {shape_id} is not a picture"))
}

// Picture reads include selected compatibility fallbacks. Ordinary mutable
// shape counts deliberately exclude those source-only representations.
fn picture_shape_id_count(children: &[ShapeTreeChild], shape_id: u32) -> usize {
    children
        .iter()
        .map(|child| match child {
            ShapeTreeChild::GroupShape(group) => {
                usize::from(child.non_visual_id() == Some(shape_id))
                    + picture_shape_id_count(&group.children, shape_id)
            }
            ShapeTreeChild::AlternateContent(alternate) => {
                picture_shape_id_count(alternate.selected_fallback().unwrap_or_default(), shape_id)
            }
            _ => usize::from(child.non_visual_id() == Some(shape_id)),
        })
        .sum()
}

fn find_picture(children: &[ShapeTreeChild], shape_id: u32) -> Option<&CT_Picture> {
    children.iter().find_map(|child| match child {
        ShapeTreeChild::Picture(picture) if child.non_visual_id() == Some(shape_id) => {
            Some(picture)
        }
        ShapeTreeChild::GroupShape(group) => find_picture(&group.children, shape_id),
        ShapeTreeChild::AlternateContent(alternate) => {
            find_picture(alternate.selected_fallback().unwrap_or_default(), shape_id)
        }
        _ => None,
    })
}

fn find_picture_mut(children: &mut [ShapeTreeChild], shape_id: u32) -> Option<&mut CT_Picture> {
    for child in children {
        let id = child.non_visual_id();
        match child {
            ShapeTreeChild::Picture(picture) if id == Some(shape_id) => return Some(picture),
            ShapeTreeChild::GroupShape(group) => {
                if let Some(picture) = find_picture_mut(&mut group.children, shape_id) {
                    return Some(picture);
                }
            }
            _ => {}
        }
    }
    None
}

fn picture_image_relationship<'a>(
    picture: &'a CT_Picture,
    operation: &'static str,
    shape_id: u32,
) -> Result<&'a str> {
    picture
        .blip_fill
        .as_ref()
        .and_then(|fill| fill.blip.as_ref())
        .and_then(|blip| blip.embed.as_deref())
        .ok_or_else(|| {
            invalid_shape_mutation(
                operation,
                format!("picture {shape_id} has no embedded image"),
            )
        })
}

fn picture_dimensions(
    image_data: &[u8],
    image_filename: &str,
    width: Option<Emu>,
    height: Option<Emu>,
) -> Result<(Emu, Emu)> {
    // The bytes are checked even when both sides are given, as every other
    // picture path checks them.
    if ImageFormat::sniff(image_data).is_none() {
        return Err(Error::UnsupportedPicture {
            filename: image_filename.to_owned(),
        });
    }
    if let (Some(width), Some(height)) = (width, height) {
        return Ok((width, height));
    }
    let info = probe(image_data).ok_or_else(|| Error::UnavailablePictureDimensions {
        filename: image_filename.to_owned(),
    })?;
    match (width, height) {
        (None, None) => info
            .native_size(72.0)
            .map(|size| (Emu(size.width_emu), Emu(size.height_emu)))
            .ok_or_else(|| Error::UnavailablePictureDimensions {
                filename: image_filename.to_owned(),
            }),
        (Some(width), None) => infer_picture_dimension(
            width,
            info.height_px,
            info.width_px,
            image_filename,
            "height",
        )
        .map(|height| (width, height)),
        (None, Some(height)) => infer_picture_dimension(
            height,
            info.width_px,
            info.height_px,
            image_filename,
            "width",
        )
        .map(|width| (width, height)),
        (Some(_), Some(_)) => unreachable!(),
    }
}

fn infer_picture_dimension(
    provided: Emu,
    inferred_pixels: u32,
    provided_pixels: u32,
    image_filename: &str,
    axis: &'static str,
) -> Result<Emu> {
    let inferred =
        i128::from(provided.0) * i128::from(inferred_pixels) / i128::from(provided_pixels);
    i64::try_from(inferred)
        .map(Emu)
        .map_err(|_| Error::PictureDimensionOutOfRange {
            filename: image_filename.to_owned(),
            axis,
        })
}

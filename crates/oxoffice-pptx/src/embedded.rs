//! PowerPoint's side of embedded content and package signatures: the
//! slides, layouts, masters and presentation part that can own OLE objects
//! and ActiveX controls, and the markup that finds their owner references.
//! The package checks live in [`oxoffice_opc::embedded`].

use std::collections::HashSet;
use std::io::Cursor;
use std::sync::Arc;

use oxoffice_core::xml_reader::NsReader;
use oxoffice_opc::OpcPackage;
use oxoffice_opc::embedded::{
    self as package_embedded, EmbeddedError, EmbeddedHost, OwnerReference, OwnerReferenceKind,
    OwnerReferences, relationship_id_attribute, required_part,
};
use oxoffice_pptx_oxml::presentation::CT_Presentation;
use oxoffice_pptx_oxml::slide_parts::{CT_Slide, CT_SlideLayout, CT_SlideMaster};
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{Namespace, ResolveResult};

use crate::{Error, Presentation, Result};

pub use oxoffice_opc::embedded::{
    EmbeddedContentInfo, EmbeddedContentKind, EmbeddedMutationPolicy, EmbeddedSignatureState,
};

const P_NS: &str = "http://schemas.openxmlformats.org/presentationml/2006/main";
const STRICT_P_NS: &str = "http://purl.oclc.org/ooxml/presentationml/main";
const A_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const STRICT_A_NS: &str = "http://purl.oclc.org/ooxml/drawingml/main";
const MC_NS: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
const OLE_GRAPHIC_DATA_URI: &str = "http://schemas.openxmlformats.org/presentationml/2006/ole";

/// Which owner references a part can hold: slides both kinds, layouts and
/// masters OLE objects only, the presentation part ActiveX controls only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OwnerPartKind {
    Slide,
    Layout,
    Master,
    Presentation,
}

#[derive(Debug)]
struct OpenNode {
    start: usize,
    is_slide_root: bool,
    is_presentation_root: bool,
    is_common_slide_data: bool,
    is_shape_tree: bool,
    is_group_shape: bool,
    is_graphic_frame: bool,
    is_graphic: bool,
    graphic_data_is_ole: bool,
    is_ole_path_container: bool,
    is_controls: bool,
    ole_relationship_ids: Vec<String>,
    control_relationship_id: Option<String>,
}

impl Presentation {
    /// Inventories relationship-owned OLE, ActiveX, and VBA payloads without decoding them.
    pub fn embedded_content(&self) -> Result<Vec<EmbeddedContentInfo>> {
        package_embedded::embedded_content(self)
    }

    /// Extracts one relationship-owned payload byte for byte.
    pub fn extract_embedded_content(
        &self,
        source_part: &str,
        relationship_id: &str,
    ) -> Result<Vec<u8>> {
        package_embedded::extract_embedded_content(self, source_part, relationship_id)
    }

    /// Replaces one opaque payload while retaining its relationship identity and part metadata.
    pub fn replace_embedded_content(
        &mut self,
        source_part: &str,
        relationship_id: &str,
        bytes: &[u8],
        policy: EmbeddedMutationPolicy,
    ) -> Result<EmbeddedContentInfo> {
        package_embedded::replace_embedded_content(
            self,
            source_part,
            relationship_id,
            bytes,
            policy,
        )
    }

    /// Removes one logical embedded object and only its newly unreachable owned candidates.
    pub fn remove_embedded_content(
        &mut self,
        source_part: &str,
        relationship_id: &str,
        policy: EmbeddedMutationPolicy,
    ) -> Result<()> {
        package_embedded::remove_embedded_content(self, source_part, relationship_id, policy)
    }

    fn owner_part_kind(&self, part: &str) -> Option<OwnerPartKind> {
        if part == self.presentation_part {
            Some(OwnerPartKind::Presentation)
        } else if self.slides.iter().any(|slide| slide.part_name == part) {
            Some(OwnerPartKind::Slide)
        } else if self.layouts.iter().any(|layout| layout.part_name == part) {
            Some(OwnerPartKind::Layout)
        } else if self.masters.iter().any(|master| master.part_name == part) {
            Some(OwnerPartKind::Master)
        } else {
            None
        }
    }

    /// The XML of an owner part as it will be written: the typed model for
    /// slides and the presentation part, the stored bytes otherwise.
    fn owner_xml(&self, part: &str, kind: OwnerPartKind) -> Result<Vec<u8>> {
        match kind {
            OwnerPartKind::Presentation => self
                .presentation
                .to_xml()
                .map_err(|error| malformed(part, error)),
            OwnerPartKind::Slide => self
                .slides
                .iter()
                .find(|slide| slide.part_name == part)
                .expect("an owner slide")
                .slide
                .to_xml()
                .map_err(|error| malformed(part, error)),
            OwnerPartKind::Layout => Ok(required_part(&self.package, part)?.to_vec()),
            OwnerPartKind::Master => {
                let xml = required_part(&self.package, part)?;
                CT_SlideMaster::from_xml(xml).map_err(|error| malformed(part, error))?;
                Ok(xml.to_vec())
            }
        }
    }

    pub(crate) fn retained_package_signature_would_be_invalidated(&self) -> Result<bool> {
        if !package_embedded::has_package_signature(&self.package)? {
            return Ok(false);
        }
        // Compare with the package as opened: slide edits written in place
        // must not hide a change from the signature.
        let staged = self.staged_package(true)?;
        let baseline = self.signature_baseline.as_deref().unwrap_or(&self.package);
        Ok(!package_embedded::packages_are_semantically_equal(
            &staged, baseline,
        ))
    }
}

impl EmbeddedHost for Presentation {
    type Error = Error;

    fn embedded_package(&self) -> &OpcPackage {
        &self.package
    }

    fn embedded_package_mut(&mut self) -> &mut OpcPackage {
        &mut self.package
    }

    fn vba_owner_part(&self) -> &str {
        &self.presentation_part
    }

    fn owner_parts(&self) -> Vec<String> {
        let mut parts = vec![self.presentation_part.clone()];
        parts.extend(self.slides.iter().map(|slide| slide.part_name.clone()));
        parts.extend(self.layouts.iter().map(|layout| layout.part_name.clone()));
        parts.extend(self.masters.iter().map(|master| master.part_name.clone()));
        parts
    }

    fn owner_references(&self, part: &str) -> Result<OwnerReferences> {
        let kind = self.owner_part_kind(part).ok_or_else(|| {
            invalid(
                "inventory embedded content",
                format!("{part}: owning XML part is outside the typed presentation model"),
            )
        })?;
        let xml = self.owner_xml(part, kind)?;
        Ok(OwnerReferences {
            ole: if kind == OwnerPartKind::Presentation {
                Vec::new()
            } else {
                xml_references(&xml, OwnerReferenceKind::Ole)?
            },
            controls: if matches!(kind, OwnerPartKind::Slide | OwnerPartKind::Presentation) {
                xml_references(&xml, OwnerReferenceKind::Control)?
            } else {
                Vec::new()
            },
        })
    }

    fn remove_owner_reference(
        &mut self,
        part: &str,
        relationship_id: &str,
        kind: OwnerReferenceKind,
    ) -> Result<()> {
        let owner = self.owner_part_kind(part).ok_or_else(|| {
            invalid(
                "remove embedded content",
                format!("{part}: owning XML part is outside the typed presentation model"),
            )
        })?;
        let xml = self.owner_xml(part, owner)?;
        let ranges = xml_references(&xml, kind)?
            .into_iter()
            .filter(|reference| reference.relationship_id == relationship_id)
            .map(|reference| reference.range)
            .collect::<Vec<_>>();
        if ranges.is_empty() {
            return Err(invalid(
                "remove embedded content",
                format!("{part}: owning XML reference {relationship_id} disappeared"),
            ));
        }
        let rewritten = package_embedded::remove_owner_ranges(&xml, ranges)?;
        match owner {
            OwnerPartKind::Presentation => {
                self.presentation = CT_Presentation::from_xml(&rewritten)
                    .map_err(|error| malformed(part, error))?;
            }
            OwnerPartKind::Slide => {
                let slide = self
                    .slides
                    .iter_mut()
                    .find(|slide| slide.part_name == part)
                    .expect("an owner slide");
                Arc::make_mut(slide).slide =
                    CT_Slide::from_xml(&rewritten).map_err(|error| malformed(part, error))?;
            }
            OwnerPartKind::Layout => {
                let layout = self
                    .layouts
                    .iter_mut()
                    .find(|layout| layout.part_name == part)
                    .expect("an owner layout");
                Arc::make_mut(layout).layout =
                    CT_SlideLayout::from_xml(&rewritten).map_err(|error| malformed(part, error))?;
                self.package.set_part(part, rewritten);
            }
            OwnerPartKind::Master => {
                CT_SlideMaster::from_xml(&rewritten).map_err(|error| malformed(part, error))?;
                self.package.set_part(part, rewritten);
            }
        }
        Ok(())
    }

    fn package_signature_invalidated(&self) -> bool {
        // An edit not yet committed invalidates a signature as much as a
        // committed one does.
        self.package_signatures_invalidated
            || self
                .retained_package_signature_would_be_invalidated()
                .unwrap_or(false)
    }

    fn payload_signature_invalidated(&self, source_part: &str, relationship_id: &str) -> bool {
        self.embedded_invalidated_signatures
            .contains(&(source_part.to_owned(), relationship_id.to_owned()))
    }

    fn invalidated_signatures_mut(&mut self) -> (&mut bool, &mut HashSet<(String, String)>) {
        (
            &mut self.package_signatures_invalidated,
            &mut self.embedded_invalidated_signatures,
        )
    }

    fn stage_candidate(&self) -> Result<Self> {
        let package = self.staged_package(true)?;
        let mut staged = Self::from_package(package)?;
        staged.embedded_invalidated_signatures = self.embedded_invalidated_signatures.clone();
        staged.package_signatures_invalidated = self.package_signatures_invalidated;
        staged.signature_baseline = self.signature_baseline.clone();
        staged.source_path = self.source_path.clone();
        Ok(staged)
    }

    fn commit_candidate(&mut self, staged: Self) -> Result<()> {
        let embedded_invalidated_signatures = staged.embedded_invalidated_signatures.clone();
        let package_signatures_invalidated = staged.package_signatures_invalidated;
        let mut package = staged.staged_package(true)?;
        persist_invalidated_package_signature(&mut package, package_signatures_invalidated)?;
        let mut output = Cursor::new(Vec::new());
        // This write is the embedded transaction's ZIP publication.
        package.write_to(&mut output)?;
        let mut reopened = Self::from_bytes(output.get_ref())?;
        // Finite XML diagnostics remain report-only for unrelated embedded edits.
        // Preserve every validation category that blocked this transaction before.
        let issues = reopened
            .validate()
            .into_iter()
            .filter(|issue| !issue.is_report_only())
            .collect::<Vec<_>>();
        if !issues.is_empty() {
            return Err(invalid(
                "mutate embedded content",
                format!("staged presentation validation failed: {issues:?}"),
            ));
        }
        reopened.embedded_invalidated_signatures = embedded_invalidated_signatures;
        reopened.package_signatures_invalidated = package_signatures_invalidated;
        reopened.signature_baseline = self.signature_baseline.clone();
        package_embedded::inventory(&reopened)?;
        reopened.source_path = self.source_path.take();
        *self = reopened;
        Ok(())
    }
}

impl From<EmbeddedError> for Error {
    fn from(error: EmbeddedError) -> Self {
        match error {
            EmbeddedError::MissingRelationship {
                source_part,
                relationship_id,
            } => Error::MissingRelationship {
                source_part,
                relationship_id,
            },
            EmbeddedError::ExternalRelationship {
                source_part,
                relationship_id,
            } => Error::ExternalRelationship {
                source_part,
                relationship_id,
            },
            EmbeddedError::MissingPart { part_name } => Error::MissingPart { part_name },
            EmbeddedError::Invalid { operation, message } => {
                Error::InvalidEmbeddedMutation { operation, message }
            }
        }
    }
}

#[cfg(feature = "digital-signatures")]
pub(crate) fn known_invalid_package_signature_on_open(package: &OpcPackage) -> bool {
    package_embedded::known_invalid_package_signature_on_open(package)
}

#[cfg(not(feature = "digital-signatures"))]
pub(crate) fn known_invalid_package_signature_on_open(package: &OpcPackage) -> bool {
    package_embedded::package_signature_known_invalid_without_verifying(package)
}

/// The package a later save is compared with, when it carries a package
/// signature.
pub(crate) fn signature_baseline(package: &OpcPackage) -> Option<Arc<OpcPackage>> {
    package_embedded::has_package_signature(package)
        .unwrap_or(false)
        .then(|| Arc::new(package.clone()))
}

pub(crate) fn persist_invalidated_package_signature(
    package: &mut OpcPackage,
    invalidated: bool,
) -> Result<()> {
    Ok(package_embedded::persist_invalidated_package_signature(
        package,
        invalidated,
    )?)
}

fn xml_references(xml: &[u8], kind: OwnerReferenceKind) -> Result<Vec<OwnerReference>> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut nodes = Vec::<OpenNode>::new();
    let mut references = Vec::new();
    loop {
        let event_start = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| invalid("scan embedded XML", error.to_string()))?;
        let is_p_namespace = namespace_is(&namespace, &[P_NS, STRICT_P_NS]);
        let is_a_namespace = namespace_is(&namespace, &[A_NS, STRICT_A_NS]);
        let is_mc_namespace = namespace_is(&namespace, &[MC_NS]);
        let event = event.into_owned();
        let event_end = reader.buffer_position() as usize;
        match event {
            Event::Start(element) => {
                let element_name = element.name();
                let local = local_name(element_name.as_ref());
                let is_slide_root =
                    is_p_namespace && matches!(local, b"sld" | b"sldLayout" | b"sldMaster");
                let is_presentation_root = is_p_namespace && local == b"presentation";
                let is_common_slide_data = is_p_namespace && local == b"cSld";
                let is_shape_tree = is_p_namespace && local == b"spTree";
                let is_group_shape = is_p_namespace && local == b"grpSp";
                let is_graphic_frame = is_p_namespace && local == b"graphicFrame";
                let is_graphic = is_a_namespace && local == b"graphic";
                let is_graphic_data = is_a_namespace && local == b"graphicData";
                let graphic_data_is_ole = is_graphic_data
                    && unqualified_attribute(&element, b"uri")?.as_deref()
                        == Some(OLE_GRAPHIC_DATA_URI);
                let is_controls = is_p_namespace && local == b"controls";
                let is_ole_path_container = is_mc_namespace
                    && matches!(local, b"AlternateContent" | b"Choice" | b"Fallback");
                let control_relationship_id = if kind == OwnerReferenceKind::Control
                    && is_p_namespace
                    && local == b"control"
                    && control_reference_is_schema_positioned(&nodes)
                {
                    relationship_id_attribute(&reader, &element)?
                } else {
                    None
                };
                if kind == OwnerReferenceKind::Ole
                    && is_p_namespace
                    && local == b"oleObj"
                    && ole_reference_is_schema_positioned(&nodes)
                    && let Some(relationship_id) = relationship_id_attribute(&reader, &element)?
                    && let Some(frame) = nodes.iter_mut().rev().find(|node| node.is_graphic_frame)
                {
                    frame.ole_relationship_ids.push(relationship_id);
                }
                nodes.push(OpenNode {
                    start: event_start,
                    is_slide_root,
                    is_presentation_root,
                    is_common_slide_data,
                    is_shape_tree,
                    is_group_shape,
                    is_graphic_frame,
                    is_graphic,
                    graphic_data_is_ole,
                    is_ole_path_container,
                    is_controls,
                    ole_relationship_ids: Vec::new(),
                    control_relationship_id,
                });
            }
            Event::Empty(element) => {
                let element_name = element.name();
                let local = local_name(element_name.as_ref());
                if kind == OwnerReferenceKind::Control
                    && is_p_namespace
                    && local == b"control"
                    && control_reference_is_schema_positioned(&nodes)
                    && let Some(relationship_id) = relationship_id_attribute(&reader, &element)?
                {
                    references.push(OwnerReference {
                        relationship_id,
                        range: event_start..event_end,
                    });
                }
                if kind == OwnerReferenceKind::Ole
                    && is_p_namespace
                    && local == b"oleObj"
                    && ole_reference_is_schema_positioned(&nodes)
                    && let Some(relationship_id) = relationship_id_attribute(&reader, &element)?
                    && let Some(frame) = nodes.iter_mut().rev().find(|node| node.is_graphic_frame)
                {
                    frame.ole_relationship_ids.push(relationship_id);
                }
            }
            Event::End(_) => {
                let node = nodes.pop().ok_or_else(|| {
                    invalid("scan embedded XML", "unmatched closing element".to_owned())
                })?;
                if kind == OwnerReferenceKind::Ole && node.is_graphic_frame {
                    let mut relationship_ids = node.ole_relationship_ids;
                    relationship_ids.sort();
                    relationship_ids.dedup();
                    if relationship_ids.len() > 1 {
                        return Err(invalid(
                            "scan embedded XML",
                            format!(
                                "graphic frame at byte {} has ambiguous OLE relationship ids {}",
                                node.start,
                                relationship_ids.join(", ")
                            ),
                        ));
                    }
                    for relationship_id in relationship_ids {
                        references.push(OwnerReference {
                            relationship_id,
                            range: node.start..event_end,
                        });
                    }
                }
                if kind == OwnerReferenceKind::Control
                    && let Some(relationship_id) = node.control_relationship_id
                {
                    references.push(OwnerReference {
                        relationship_id,
                        range: node.start..event_end,
                    });
                }
            }
            Event::Eof => {
                if !nodes.is_empty() {
                    return Err(invalid(
                        "scan embedded XML",
                        "unclosed XML element".to_owned(),
                    ));
                }
                references.sort_by_key(|reference| reference.range.start);
                return Ok(references);
            }
            _ => {}
        }
        buffer.clear();
    }
}

fn ole_reference_is_schema_positioned(nodes: &[OpenNode]) -> bool {
    let Some(graphic_data) = nodes.iter().rposition(|node| node.graphic_data_is_ole) else {
        return false;
    };
    if graphic_data < 2
        || !nodes[graphic_data - 1].is_graphic
        || !nodes[graphic_data - 2].is_graphic_frame
        || !schema_owned_graphic_frame(nodes, graphic_data - 2)
    {
        return false;
    }
    nodes[graphic_data + 1..]
        .iter()
        .all(|node| node.is_ole_path_container)
}

fn schema_owned_graphic_frame(nodes: &[OpenNode], frame: usize) -> bool {
    let Some(mut parent) = frame.checked_sub(1) else {
        return false;
    };
    while nodes[parent].is_ole_path_container {
        let Some(next) = parent.checked_sub(1) else {
            return false;
        };
        parent = next;
    }
    while nodes[parent].is_group_shape {
        let Some(next) = parent.checked_sub(1) else {
            return false;
        };
        parent = next;
        while nodes[parent].is_ole_path_container {
            let Some(next) = parent.checked_sub(1) else {
                return false;
            };
            parent = next;
        }
    }
    if !nodes[parent].is_shape_tree || parent < 2 {
        return false;
    }
    nodes[parent - 1].is_common_slide_data && nodes[parent - 2].is_slide_root
}

fn control_reference_is_schema_positioned(nodes: &[OpenNode]) -> bool {
    let Some(controls) = nodes.last() else {
        return false;
    };
    if !controls.is_controls || nodes.len() < 2 {
        return false;
    }
    let mut parent = nodes.len() - 2;
    while nodes[parent].is_ole_path_container {
        let Some(next) = parent.checked_sub(1) else {
            return false;
        };
        parent = next;
    }
    (nodes[parent].is_common_slide_data && parent == 1 && nodes[0].is_slide_root)
        || (nodes[parent].is_presentation_root && parent == 0)
}

fn unqualified_attribute(element: &BytesStart<'_>, name: &[u8]) -> Result<Option<String>> {
    for attribute in element.attributes() {
        let attribute =
            attribute.map_err(|error| invalid("scan embedded XML", error.to_string()))?;
        if attribute.key.as_ref() == name {
            return attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                .map(|value| Some(value.into_owned()))
                .map_err(|error| invalid("scan embedded XML", error.to_string()));
        }
    }
    Ok(None)
}

fn namespace_is(namespace: &ResolveResult<'_>, expected: &[&str]) -> bool {
    matches!(namespace, ResolveResult::Bound(Namespace(uri)) if expected.iter().any(|value| *uri == value.as_bytes()))
}

fn local_name(name: &[u8]) -> &[u8] {
    name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
}

fn malformed(part_name: &str, error: impl std::fmt::Display) -> Error {
    Error::MalformedPart {
        part_name: part_name.to_owned(),
        message: error.to_string(),
    }
}

fn invalid(operation: &'static str, message: String) -> Error {
    Error::InvalidEmbeddedMutation { operation, message }
}

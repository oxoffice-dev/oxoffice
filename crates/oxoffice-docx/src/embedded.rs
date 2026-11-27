//! Word's side of embedded content and package signatures: the stories
//! that can own OLE objects and ActiveX controls, and the markup grammar
//! that finds their owner references. The package checks live in
//! [`oxoffice_opc::embedded`].

use std::collections::{BTreeSet, HashSet};
use std::ops::Range;

use oxoffice_core::xml::xml_ncname_is_valid;
use oxoffice_core::xml_reader::NsReader;
use oxoffice_opc::OpcPackage;
use oxoffice_opc::embedded::{
    self as package_embedded, EmbeddedError, EmbeddedHost, OwnerReference, OwnerReferenceKind,
    OwnerReferences, relationship_id_attribute, required_part, validate_embedded_lexical,
    validate_utf8_xml_declaration,
};
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use quick_xml::name::{Namespace, QName, ResolveResult};

use crate::{Document, Error, Result};

pub use oxoffice_opc::embedded::{
    EmbeddedContentInfo, EmbeddedContentKind, EmbeddedMutationPolicy, EmbeddedSignatureState,
};
pub(crate) use oxoffice_opc::embedded::{
    relationship_target_is_normalized_pack_uri, synchronized_package_mutation_invalidates_signature,
};

const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const STRICT_W_NS: &str = "http://purl.oclc.org/ooxml/wordprocessingml/main";
const O_NS: &str = "urn:schemas-microsoft-com:office:office";
const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const STRICT_R_NS: &str = "http://purl.oclc.org/ooxml/officeDocument/relationships";
const MC_NS: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";
const VML_NS: &str = "urn:schemas-microsoft-com:vml";
const WP_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
const A_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const WPG_NS: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingGroup";
const WPS_NS: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingShape";
const WPC_NS: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingCanvas";

#[derive(Debug)]
struct OpenNode {
    start: usize,
    is_document_element: bool,
    story_path_kind: StoryPathKind,
    is_paragraph: bool,
    is_run: bool,
    is_object: bool,
    is_control_owner: bool,
    mc_path_kind: McPathKind,
    mc_branch_valid: bool,
    mc_container_state: Option<McContainerState>,
    mc_rules_valid: bool,
    ignorable_namespaces: BTreeSet<Vec<u8>>,
    has_invalid_mc_descendant: bool,
    text_box_path_kind: TextBoxPathKind,
    run_owner_kind: RunOwnerKind,
    object_relationship_ids: Vec<String>,
    object_child_count: usize,
    control_relationship_id: Option<String>,
    control_child_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RunOwnerKind {
    Other,
    Container,
    StructuredDocumentTag,
    StructuredDocumentTagContent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum McPathKind {
    Other,
    AlternateContent,
    Choice,
    Fallback,
}

#[derive(Clone, Copy, Debug)]
struct McContainerState {
    choice_count: usize,
    fallback_seen: bool,
    grammar_valid: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TextBoxPathKind {
    Other,
    LegacyPicture,
    LegacyObject,
    VmlGroup,
    VmlShape,
    VmlTextBox,
    Drawing,
    WordprocessingDrawing,
    Graphic,
    GraphicDataShape,
    GraphicDataGroup,
    GraphicDataCanvas,
    WordprocessingGroup,
    WordprocessingCanvas,
    WordprocessingShape,
    WordprocessingTextBox,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StoryRootKind {
    Document,
    Header,
    Footer,
    Footnotes,
    Endnotes,
    Comments,
    Glossary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TextBoxNamespaceKind {
    Other,
    Word,
    Vml,
    WordprocessingDrawing,
    Drawing,
    WordprocessingGroup,
    WordprocessingShape,
    WordprocessingCanvas,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StoryPathKind {
    Other,
    DocumentRoot,
    HeaderFooterRoot,
    FootnotesRoot,
    EndnotesRoot,
    CommentsRoot,
    GlossaryRoot,
    TextBoxRoot,
    Body,
    Footnote,
    Endnote,
    Comment,
    DocParts,
    DocPart,
    DocPartBody,
    CustomXml,
    StructuredDocumentTag,
    StructuredDocumentTagContent,
    Table,
    TableRow,
    TableCell,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StoryContentKind {
    Block,
    TableRow,
    TableCell,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExpectedStoryChild {
    Body,
    Footnote,
    Endnote,
    Comment,
    DocParts,
    DocPart,
    DocPartBody,
    Content(StoryContentKind),
    StructuredDocumentTagContent(StoryContentKind),
}

impl Document {
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

    fn word_story_root_kind(&self, part_name: &str) -> Option<StoryRootKind> {
        if part_name == self.doc_part_name {
            return Some(StoryRootKind::Document);
        }
        match self.package.content_types.content_type_for(part_name)? {
            "application/vnd.openxmlformats-officedocument.wordprocessingml.header+xml" => {
                Some(StoryRootKind::Header)
            }
            "application/vnd.openxmlformats-officedocument.wordprocessingml.footer+xml" => {
                Some(StoryRootKind::Footer)
            }
            "application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml" => {
                Some(StoryRootKind::Footnotes)
            }
            "application/vnd.openxmlformats-officedocument.wordprocessingml.endnotes+xml" => {
                Some(StoryRootKind::Endnotes)
            }
            "application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml" => {
                Some(StoryRootKind::Comments)
            }
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document.glossary+xml" => {
                Some(StoryRootKind::Glossary)
            }
            _ => None,
        }
    }

    pub(crate) fn retained_package_signature_would_be_invalidated(&self) -> Result<bool> {
        if !package_embedded::has_package_signature(&self.package)? {
            return Ok(false);
        }
        let mut staged = self.clone_for_staging();
        staged.flush_to_package()?;
        if self
            .package
            .get_part(&self.doc_part_name)
            .and_then(|xml| oxoffice_docx_oxml::document::CT_Document::from_xml(xml).ok())
            .as_ref()
            == Some(&self.document)
            && let Some(original) = self.package.get_part(&self.doc_part_name)
        {
            staged
                .package
                .set_part(&self.doc_part_name, original.to_vec());
        }
        Ok(!package_embedded::packages_are_semantically_equal(
            &staged.package,
            &self.package,
        ))
    }
}

impl EmbeddedHost for Document {
    type Error = Error;

    fn embedded_package(&self) -> &OpcPackage {
        &self.package
    }

    fn embedded_package_mut(&mut self) -> &mut OpcPackage {
        &mut self.package
    }

    fn vba_owner_part(&self) -> &str {
        &self.doc_part_name
    }

    fn owner_parts(&self) -> Vec<String> {
        self.package
            .part_rels
            .keys()
            .chain(self.package.parts.keys())
            .filter(|part| self.word_story_root_kind(part).is_some())
            .cloned()
            .collect()
    }

    fn owner_references(&self, part: &str) -> Result<OwnerReferences> {
        let xml = required_part(&self.package, part)?;
        let story_root_kind = self.word_story_root_kind(part).ok_or_else(|| {
            invalid(
                "inventory embedded content",
                format!("{part}: unsupported Word story part"),
            )
        })?;
        Ok(OwnerReferences {
            ole: xml_references(xml, OwnerReferenceKind::Ole, story_root_kind)?,
            controls: xml_references(xml, OwnerReferenceKind::Control, story_root_kind)?,
        })
    }

    fn remove_owner_reference(
        &mut self,
        part: &str,
        relationship_id: &str,
        kind: OwnerReferenceKind,
    ) -> Result<()> {
        let story_root_kind = self.word_story_root_kind(part).ok_or_else(|| {
            invalid(
                "remove embedded content",
                format!("{part}: unsupported Word story part"),
            )
        })?;
        let xml = required_part(&self.package, part)?.to_vec();
        let ranges = xml_references(&xml, kind, story_root_kind)?
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
        self.package.set_part(part, rewritten);
        Ok(())
    }

    fn package_signature_invalidated(&self) -> bool {
        self.package_signatures_invalidated
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
        let mut staged = self.clone_for_staging();
        staged.prepare_staged_package()?;
        Ok(staged)
    }

    fn commit_candidate(&mut self, mut staged: Self) -> Result<()> {
        persist_invalidated_package_signature(
            &mut staged.package,
            staged.package_signatures_invalidated,
        )?;
        package_embedded::inventory(&staged)?;
        let reopened = staged.reopen_prepared_staged()?;
        package_embedded::inventory(&reopened)?;
        self.commit_staged_mutation(reopened);
        Ok(())
    }
}

impl From<EmbeddedError> for Error {
    fn from(error: EmbeddedError) -> Self {
        Error::InvalidEmbeddedMutation {
            operation: error.operation(),
            message: error.message(),
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

pub(crate) fn persist_invalidated_package_signature(
    package: &mut OpcPackage,
    invalidated: bool,
) -> Result<()> {
    Ok(package_embedded::persist_invalidated_package_signature(
        package,
        invalidated,
    )?)
}

fn xml_references(
    xml: &[u8],
    kind: OwnerReferenceKind,
    expected_root: StoryRootKind,
) -> Result<Vec<OwnerReference>> {
    validate_embedded_lexical(xml, "scan embedded XML")?;
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut nodes = Vec::<OpenNode>::new();
    let mut references = Vec::new();
    let mut invalid_mc_ranges = Vec::<Range<usize>>::new();
    let mut document_element_count = 0usize;
    let mut document_element_is_expected = false;
    let mut outside_document_content_seen = false;
    let mut declaration_seen = false;
    let mut prolog_content_seen = false;
    loop {
        let event_start = reader.buffer_position() as usize;
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| invalid("scan embedded XML", error.to_string()))?;
        let is_word = namespace_is(&namespace, &[W_NS, STRICT_W_NS]);
        let is_office = namespace_is(&namespace, &[O_NS]);
        let is_mc = namespace_is(&namespace, &[MC_NS]);
        let is_vml = namespace_is(&namespace, &[VML_NS]);
        let is_wordprocessing_drawing = namespace_is(&namespace, &[WP_NS]);
        let is_drawing = namespace_is(&namespace, &[A_NS]);
        let is_wordprocessing_group = namespace_is(&namespace, &[WPG_NS]);
        let is_wordprocessing_shape = namespace_is(&namespace, &[WPS_NS]);
        let is_wordprocessing_canvas = namespace_is(&namespace, &[WPC_NS]);
        let text_box_namespace_kind = if is_word {
            TextBoxNamespaceKind::Word
        } else if is_vml {
            TextBoxNamespaceKind::Vml
        } else if is_wordprocessing_drawing {
            TextBoxNamespaceKind::WordprocessingDrawing
        } else if is_drawing {
            TextBoxNamespaceKind::Drawing
        } else if is_wordprocessing_group {
            TextBoxNamespaceKind::WordprocessingGroup
        } else if is_wordprocessing_shape {
            TextBoxNamespaceKind::WordprocessingShape
        } else if is_wordprocessing_canvas {
            TextBoxNamespaceKind::WordprocessingCanvas
        } else {
            TextBoxNamespaceKind::Other
        };
        let resolved_namespace = match &namespace {
            ResolveResult::Bound(Namespace(uri)) => Some(uri.to_vec()),
            _ => None,
        };
        let event = event.into_owned();
        drop(namespace);
        let event_end = reader.buffer_position() as usize;
        match event {
            Event::Start(element) => {
                let is_document_element = nodes.is_empty()
                    && document_element_count == 0
                    && !outside_document_content_seen;
                if nodes.is_empty() {
                    document_element_count = document_element_count.saturating_add(1);
                }
                let element_name = element.name();
                let local = local_name(element_name.as_ref());
                if is_document_element {
                    document_element_is_expected =
                        story_root_matches(expected_root, is_word, local);
                }
                let is_text_box_story_root = is_word
                    && local == b"txbxContent"
                    && effective_text_box_parent(&nodes).is_some_and(|parent| {
                        matches!(
                            parent,
                            TextBoxPathKind::VmlTextBox | TextBoxPathKind::WordprocessingTextBox
                        )
                    });
                let story_path_kind = story_path_kind(is_word, local, is_text_box_story_root);
                let is_paragraph = is_word && local == b"p";
                let is_run = is_word && local == b"r";
                let run_owner_kind = run_owner_kind(is_word, local);
                let graphic_data_uri = if is_drawing && local == b"graphicData" {
                    drawing_graphic_data_uri(&reader, &element)?
                } else {
                    None
                };
                let text_box_path_kind = text_box_path_kind(
                    text_box_namespace_kind,
                    local,
                    graphic_data_uri.as_deref(),
                    &nodes,
                );
                let mc_path_kind = if is_mc {
                    match local {
                        b"AlternateContent" => McPathKind::AlternateContent,
                        b"Choice" => McPathKind::Choice,
                        b"Fallback" => McPathKind::Fallback,
                        _ => McPathKind::Other,
                    }
                } else {
                    McPathKind::Other
                };
                let inherited_ignorable_namespaces = nodes
                    .last()
                    .map(|node| &node.ignorable_namespaces)
                    .cloned()
                    .unwrap_or_default();
                let (mc_rules_valid, ignorable_namespaces) =
                    mc_rule_state(&reader, &element, &inherited_ignorable_namespaces)?;
                let mc_element_valid = mc_path_kind == McPathKind::Other
                    || mc_element_attributes_are_valid(
                        &reader,
                        &element,
                        mc_path_kind,
                        &ignorable_namespaces,
                    )?;
                let ignorable_extension_child = mc_path_kind == McPathKind::Other
                    && resolved_namespace
                        .as_deref()
                        .is_some_and(|uri| inherited_ignorable_namespaces.contains(uri));
                let mc_branch_valid = record_mc_child(
                    &reader,
                    &element,
                    mc_path_kind,
                    mc_rules_valid && mc_element_valid,
                    ignorable_extension_child,
                    &mut nodes,
                )?;
                let is_object = is_word
                    && local == b"object"
                    && mc_rules_valid
                    && run_child_is_schema_positioned(&nodes);
                let is_control_owner = is_word
                    && matches!(local, b"object" | b"pict")
                    && mc_rules_valid
                    && run_child_is_schema_positioned(&nodes);
                if is_word && local == b"control" && control_child_is_schema_positioned(&nodes) {
                    let relationship_id = if mc_rules_valid {
                        relationship_id_attribute(&reader, &element)?
                    } else {
                        None
                    };
                    record_control_child(&mut nodes, relationship_id)?;
                }
                if is_office && local == b"OLEObject" && object_child_is_schema_positioned(&nodes) {
                    let relationship_id = if mc_rules_valid {
                        relationship_id_attribute(&reader, &element)?
                    } else {
                        None
                    };
                    record_object_child(&mut nodes, relationship_id)?;
                }
                nodes.push(OpenNode {
                    start: event_start,
                    is_document_element,
                    story_path_kind,
                    is_paragraph,
                    is_run,
                    is_object,
                    is_control_owner,
                    mc_path_kind,
                    mc_branch_valid,
                    mc_container_state: (mc_path_kind == McPathKind::AlternateContent).then_some(
                        McContainerState {
                            choice_count: 0,
                            fallback_seen: false,
                            grammar_valid: mc_rules_valid && mc_element_valid,
                        },
                    ),
                    mc_rules_valid,
                    ignorable_namespaces,
                    has_invalid_mc_descendant: false,
                    text_box_path_kind,
                    run_owner_kind,
                    object_relationship_ids: Vec::new(),
                    object_child_count: 0,
                    control_relationship_id: None,
                    control_child_count: 0,
                });
            }
            Event::Empty(element) => {
                let is_document_element = nodes.is_empty()
                    && document_element_count == 0
                    && !outside_document_content_seen;
                if nodes.is_empty() {
                    document_element_count = document_element_count.saturating_add(1);
                }
                let element_name = element.name();
                let local = local_name(element_name.as_ref());
                if is_document_element {
                    document_element_is_expected =
                        story_root_matches(expected_root, is_word, local);
                }
                let mc_path_kind = if is_mc {
                    match local {
                        b"AlternateContent" => McPathKind::AlternateContent,
                        b"Choice" => McPathKind::Choice,
                        b"Fallback" => McPathKind::Fallback,
                        _ => McPathKind::Other,
                    }
                } else {
                    McPathKind::Other
                };
                let inherited_ignorable_namespaces = nodes
                    .last()
                    .map(|node| &node.ignorable_namespaces)
                    .cloned()
                    .unwrap_or_default();
                let (mc_rules_valid, ignorable_namespaces) =
                    mc_rule_state(&reader, &element, &inherited_ignorable_namespaces)?;
                let mc_element_valid = mc_path_kind == McPathKind::Other
                    || mc_element_attributes_are_valid(
                        &reader,
                        &element,
                        mc_path_kind,
                        &ignorable_namespaces,
                    )?;
                let ignorable_extension_child = mc_path_kind == McPathKind::Other
                    && resolved_namespace
                        .as_deref()
                        .is_some_and(|uri| inherited_ignorable_namespaces.contains(uri));
                record_mc_child(
                    &reader,
                    &element,
                    mc_path_kind,
                    mc_rules_valid && mc_element_valid,
                    ignorable_extension_child,
                    &mut nodes,
                )?;
                if is_word && local == b"control" && control_child_is_schema_positioned(&nodes) {
                    let relationship_id = if mc_rules_valid {
                        relationship_id_attribute(&reader, &element)?
                    } else {
                        None
                    };
                    record_control_child(&mut nodes, relationship_id)?;
                }
                if is_office && local == b"OLEObject" && object_child_is_schema_positioned(&nodes) {
                    let relationship_id = if mc_rules_valid {
                        relationship_id_attribute(&reader, &element)?
                    } else {
                        None
                    };
                    record_object_child(&mut nodes, relationship_id)?;
                }
            }
            Event::End(_) => {
                let node = nodes.pop().ok_or_else(|| {
                    invalid("scan embedded XML", "unmatched closing element".to_owned())
                })?;
                if node
                    .mc_container_state
                    .is_some_and(|state| !state.grammar_valid || state.choice_count == 0)
                {
                    invalid_mc_ranges.push(node.start..event_end);
                    for ancestor in &mut nodes {
                        if ancestor.is_object || ancestor.is_control_owner {
                            ancestor.has_invalid_mc_descendant = true;
                        }
                    }
                }
                if node.is_object
                    && !node.has_invalid_mc_descendant
                    && node.object_child_count > 0
                    && node.control_child_count > 0
                {
                    return Err(invalid(
                        "scan embedded XML",
                        format!(
                            "Word object at byte {} mixes OLE and ActiveX owner children",
                            node.start
                        ),
                    ));
                }
                if kind == OwnerReferenceKind::Ole
                    && node.is_object
                    && !node.has_invalid_mc_descendant
                {
                    let relationship_ids = node.object_relationship_ids;
                    if node.object_child_count > 1 {
                        return Err(invalid(
                            "scan embedded XML",
                            format!(
                                "Word object at byte {} has ambiguous OLE relationship ids {}",
                                node.start,
                                relationship_ids.join(", ")
                            ),
                        ));
                    }
                    if node.object_child_count == 1 && relationship_ids.is_empty() {
                        return Err(invalid(
                            "scan embedded XML",
                            format!(
                                "Word object at byte {} has an OLE child without a relationship id",
                                node.start
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
                    && node.is_control_owner
                    && !node.has_invalid_mc_descendant
                    && node.control_child_count > 1
                {
                    return Err(invalid(
                        "scan embedded XML",
                        format!(
                            "Word control owner at byte {} has {} control children",
                            node.start, node.control_child_count
                        ),
                    ));
                }
                if kind == OwnerReferenceKind::Control
                    && node.is_control_owner
                    && !node.has_invalid_mc_descendant
                    && node.control_child_count == 1
                    && node.control_relationship_id.is_none()
                {
                    return Err(invalid(
                        "scan embedded XML",
                        format!(
                            "Word control owner at byte {} has a child without a relationship id",
                            node.start
                        ),
                    ));
                }
                if kind == OwnerReferenceKind::Control
                    && node.is_control_owner
                    && !node.has_invalid_mc_descendant
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
                if outside_document_content_seen
                    || document_element_count != 1
                    || !document_element_is_expected
                {
                    return Ok(Vec::new());
                }
                references.retain(|reference| {
                    !invalid_mc_ranges.iter().any(|range| {
                        range.start <= reference.range.start && reference.range.end <= range.end
                    })
                });
                references.sort_by_key(|reference| reference.range.start);
                return Ok(references);
            }
            Event::Text(text) if nodes.is_empty() => {
                let bytes: &[u8] = text.as_ref();
                outside_document_content_seen |= !bytes.iter().all(u8::is_ascii_whitespace);
                if document_element_count == 0 {
                    prolog_content_seen = true;
                }
            }
            Event::CData(_) | Event::GeneralRef(_) if nodes.is_empty() => {
                outside_document_content_seen = true;
            }
            Event::GeneralRef(reference) => {
                let non_whitespace = reference
                    .resolve_char_ref()
                    .ok()
                    .flatten()
                    .is_none_or(|character| !character.is_ascii_whitespace());
                if nodes.last().is_some_and(|node| {
                    node.mc_path_kind == McPathKind::AlternateContent && non_whitespace
                }) {
                    nodes
                        .last_mut()
                        .unwrap()
                        .mc_container_state
                        .as_mut()
                        .unwrap()
                        .grammar_valid = false;
                }
            }
            Event::Decl(declaration) => {
                let misplaced = !nodes.is_empty()
                    || document_element_count != 0
                    || declaration_seen
                    || prolog_content_seen;
                if misplaced {
                    outside_document_content_seen = true;
                } else {
                    validate_utf8_xml_declaration(&declaration, "scan embedded XML")?;
                }
                declaration_seen = true;
            }
            Event::DocType(_) => {
                return Err(invalid(
                    "scan embedded XML",
                    "Word story XML cannot contain a document type".to_owned(),
                ));
            }
            Event::Comment(_) | Event::PI(_) if nodes.is_empty() && document_element_count == 0 => {
                prolog_content_seen = true;
            }
            Event::Text(text)
                if nodes.last().is_some_and(|node| {
                    node.mc_path_kind == McPathKind::AlternateContent && {
                        let bytes: &[u8] = text.as_ref();
                        !bytes.iter().all(u8::is_ascii_whitespace)
                    }
                }) =>
            {
                nodes
                    .last_mut()
                    .unwrap()
                    .mc_container_state
                    .as_mut()
                    .unwrap()
                    .grammar_valid = false;
            }
            Event::CData(_)
                if nodes
                    .last()
                    .is_some_and(|node| node.mc_path_kind == McPathKind::AlternateContent) =>
            {
                nodes
                    .last_mut()
                    .unwrap()
                    .mc_container_state
                    .as_mut()
                    .unwrap()
                    .grammar_valid = false;
            }
            _ => {}
        }
        buffer.clear();
    }
}

fn record_mc_child(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
    child_kind: McPathKind,
    element_valid: bool,
    ignorable_extension_child: bool,
    nodes: &mut [OpenNode],
) -> Result<bool> {
    let Some(state) = nodes
        .last_mut()
        .and_then(|node| node.mc_container_state.as_mut())
    else {
        return Ok(false);
    };
    match child_kind {
        McPathKind::Choice => {
            let branch_valid = state.grammar_valid
                && !state.fallback_seen
                && element_valid
                && choice_has_valid_requires(reader, element)?;
            state.choice_count = state.choice_count.saturating_add(1);
            state.grammar_valid &= branch_valid;
            Ok(branch_valid)
        }
        McPathKind::Fallback => {
            let branch_valid = state.grammar_valid
                && state.choice_count > 0
                && !state.fallback_seen
                && element_valid
                && fallback_has_no_requires(reader, element)?;
            state.fallback_seen = true;
            state.grammar_valid &= branch_valid;
            Ok(branch_valid)
        }
        McPathKind::Other if ignorable_extension_child => Ok(false),
        McPathKind::Other | McPathKind::AlternateContent => {
            state.grammar_valid = false;
            Ok(false)
        }
    }
}

fn choice_has_valid_requires(reader: &NsReader<&[u8]>, element: &BytesStart<'_>) -> Result<bool> {
    let mut requires = None;
    for attribute in element.attributes() {
        let attribute =
            attribute.map_err(|error| invalid("scan embedded XML", error.to_string()))?;
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        if matches!(namespace, ResolveResult::Unbound) && local.as_ref() == b"Requires" {
            if requires.is_some() {
                return Ok(false);
            }
            requires = Some(
                attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                    .map_err(|error| invalid("scan embedded XML", error.to_string()))?
                    .into_owned(),
            );
        }
    }
    Ok(requires.is_some_and(|value| {
        let mut prefixes = value.split_ascii_whitespace();
        prefixes.clone().next().is_some()
            && prefixes.all(|prefix| namespace_prefix_is_valid(reader, prefix))
    }))
}

fn fallback_has_no_requires(reader: &NsReader<&[u8]>, element: &BytesStart<'_>) -> Result<bool> {
    for attribute in element.attributes() {
        let attribute =
            attribute.map_err(|error| invalid("scan embedded XML", error.to_string()))?;
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        if matches!(namespace, ResolveResult::Unbound) && local.as_ref() == b"Requires" {
            return Ok(false);
        }
    }
    Ok(true)
}

fn mc_rule_state(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
    inherited_ignorable: &BTreeSet<Vec<u8>>,
) -> Result<(bool, BTreeSet<Vec<u8>>)> {
    let mut ignorable = inherited_ignorable.clone();
    let mut valid = true;
    let mut seen = HashSet::<Vec<u8>>::new();

    for attribute in element.attributes() {
        let attribute =
            attribute.map_err(|error| invalid("scan embedded XML", error.to_string()))?;
        let raw_name = attribute.key.as_ref();
        if raw_name == b"xmlns" || raw_name.starts_with(b"xmlns:") {
            continue;
        }
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        if !namespace_is(&namespace, &[MC_NS]) {
            continue;
        }
        if !seen.insert(local.as_ref().to_vec()) {
            valid = false;
            continue;
        }
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
            .map_err(|error| invalid("scan embedded XML", error.to_string()))?;
        if local.as_ref() == b"Ignorable" {
            for prefix in value.split_ascii_whitespace() {
                let Some(namespace) = namespace_for_prefix(reader, prefix) else {
                    valid = false;
                    continue;
                };
                ignorable.insert(namespace);
            }
        }
    }

    for attribute in element.attributes() {
        let attribute =
            attribute.map_err(|error| invalid("scan embedded XML", error.to_string()))?;
        let raw_name = attribute.key.as_ref();
        if raw_name == b"xmlns" || raw_name.starts_with(b"xmlns:") {
            continue;
        }
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        if !namespace_is(&namespace, &[MC_NS]) {
            continue;
        }
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
            .map_err(|error| invalid("scan embedded XML", error.to_string()))?;
        let attribute_valid = match local.as_ref() {
            b"Ignorable" => true,
            b"MustUnderstand" => value
                .split_ascii_whitespace()
                .all(|prefix| must_understand_prefix_is_valid(reader, prefix)),
            b"ProcessContent" => mc_qname_list_is_valid(reader, &value, true, Some(&ignorable)),
            b"PreserveElements" | b"PreserveAttributes" => {
                mc_qname_list_is_valid(reader, &value, true, Some(&ignorable))
            }
            _ => false,
        };
        valid &= attribute_valid;
    }
    Ok((valid, ignorable))
}

fn mc_element_attributes_are_valid(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
    element_kind: McPathKind,
    ignorable_namespaces: &BTreeSet<Vec<u8>>,
) -> Result<bool> {
    for attribute in element.attributes() {
        let attribute =
            attribute.map_err(|error| invalid("scan embedded XML", error.to_string()))?;
        let raw_name = attribute.key.as_ref();
        if raw_name == b"xmlns" || raw_name.starts_with(b"xmlns:") {
            continue;
        }
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        if namespace_is(&namespace, &[XML_NS]) {
            return Ok(false);
        }
        if matches!(namespace, ResolveResult::Unbound) {
            if element_kind == McPathKind::Choice
                && !raw_name.contains(&b':')
                && local.as_ref() == b"Requires"
            {
                continue;
            }
            return Ok(false);
        }
        if namespace_is(&namespace, &[MC_NS])
            && !matches!(
                local.as_ref(),
                b"Ignorable"
                    | b"MustUnderstand"
                    | b"ProcessContent"
                    | b"PreserveElements"
                    | b"PreserveAttributes"
            )
        {
            return Ok(false);
        }
        if !namespace_is(&namespace, &[MC_NS])
            && !resolved_namespace_is_ignorable(&namespace, ignorable_namespaces)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn namespace_prefix_is_valid(reader: &NsReader<&[u8]>, prefix: &str) -> bool {
    namespace_for_prefix(reader, prefix).is_some()
}

pub(crate) fn must_understand_prefix_is_valid(reader: &NsReader<&[u8]>, prefix: &str) -> bool {
    namespace_for_prefix(reader, prefix).is_some_and(|namespace| {
        [
            W_NS,
            STRICT_W_NS,
            O_NS,
            R_NS,
            STRICT_R_NS,
            VML_NS,
            WP_NS,
            A_NS,
            WPG_NS,
            WPS_NS,
            WPC_NS,
        ]
        .iter()
        .any(|understood| namespace == understood.as_bytes())
    })
}

fn namespace_for_prefix(reader: &NsReader<&[u8]>, prefix: &str) -> Option<Vec<u8>> {
    if !xml_ncname_is_valid(prefix) {
        return None;
    }
    let qualified_name = format!("{prefix}:mcProbe");
    match reader
        .resolver()
        .resolve_element(QName(qualified_name.as_bytes()))
        .0
    {
        ResolveResult::Bound(Namespace(namespace))
            if namespace != MC_NS.as_bytes() && namespace != XML_NS.as_bytes() =>
        {
            Some(namespace.to_vec())
        }
        _ => None,
    }
}

fn mc_qname_list_is_valid(
    reader: &NsReader<&[u8]>,
    value: &str,
    wildcard_allowed: bool,
    required_ignorable: Option<&BTreeSet<Vec<u8>>>,
) -> bool {
    value.split_ascii_whitespace().all(|token| {
        let mut parts = token.split(':');
        let (Some(prefix), Some(local), None) = (parts.next(), parts.next(), parts.next()) else {
            return false;
        };
        let Some(namespace) = namespace_for_prefix(reader, prefix) else {
            return false;
        };
        required_ignorable.is_none_or(|ignorable| ignorable.contains(&namespace))
            && ((wildcard_allowed && local == "*") || xml_ncname_is_valid(local))
    })
}

fn run_child_is_schema_positioned(nodes: &[OpenNode]) -> bool {
    if !compatibility_ancestry_is_valid(nodes) {
        return false;
    }
    let Some(mut parent) = nodes.len().checked_sub(1) else {
        return false;
    };
    while nodes[parent].mc_path_kind != McPathKind::Other {
        let Some(next) = parent.checked_sub(1) else {
            return false;
        };
        parent = next;
    }
    if !nodes[parent].is_run {
        return false;
    }
    let Some(paragraph) = nodes[..parent].iter().rposition(|node| node.is_paragraph) else {
        return false;
    };
    valid_story_owner_path(&nodes[..paragraph])
        && valid_run_owner_path(&nodes[paragraph + 1..parent])
}

fn story_path_kind(is_word: bool, local: &[u8], is_text_box_story_root: bool) -> StoryPathKind {
    if !is_word {
        return StoryPathKind::Other;
    }
    if is_text_box_story_root {
        return StoryPathKind::TextBoxRoot;
    }
    match local {
        b"document" => StoryPathKind::DocumentRoot,
        b"hdr" | b"ftr" => StoryPathKind::HeaderFooterRoot,
        b"footnotes" => StoryPathKind::FootnotesRoot,
        b"endnotes" => StoryPathKind::EndnotesRoot,
        b"comments" => StoryPathKind::CommentsRoot,
        b"glossaryDocument" => StoryPathKind::GlossaryRoot,
        b"body" => StoryPathKind::Body,
        b"footnote" => StoryPathKind::Footnote,
        b"endnote" => StoryPathKind::Endnote,
        b"comment" => StoryPathKind::Comment,
        b"docParts" => StoryPathKind::DocParts,
        b"docPart" => StoryPathKind::DocPart,
        b"docPartBody" => StoryPathKind::DocPartBody,
        b"customXml" => StoryPathKind::CustomXml,
        b"sdt" => StoryPathKind::StructuredDocumentTag,
        b"sdtContent" => StoryPathKind::StructuredDocumentTagContent,
        b"tbl" => StoryPathKind::Table,
        b"tr" => StoryPathKind::TableRow,
        b"tc" => StoryPathKind::TableCell,
        _ => StoryPathKind::Other,
    }
}

fn story_root_matches(expected: StoryRootKind, is_word: bool, local: &[u8]) -> bool {
    is_word
        && matches!(
            (expected, local),
            (StoryRootKind::Document, b"document")
                | (StoryRootKind::Header, b"hdr")
                | (StoryRootKind::Footer, b"ftr")
                | (StoryRootKind::Footnotes, b"footnotes")
                | (StoryRootKind::Endnotes, b"endnotes")
                | (StoryRootKind::Comments, b"comments")
                | (StoryRootKind::Glossary, b"glossaryDocument")
        )
}

fn valid_story_owner_path(nodes: &[OpenNode]) -> bool {
    let Some(root_index) = nodes
        .iter()
        .rposition(|node| node.story_path_kind == StoryPathKind::TextBoxRoot)
        .or_else(|| {
            nodes.first().and_then(|node| {
                (node.is_document_element
                    && matches!(
                        node.story_path_kind,
                        StoryPathKind::DocumentRoot
                            | StoryPathKind::HeaderFooterRoot
                            | StoryPathKind::FootnotesRoot
                            | StoryPathKind::EndnotesRoot
                            | StoryPathKind::CommentsRoot
                            | StoryPathKind::GlossaryRoot
                    ))
                .then_some(0)
            })
        })
    else {
        return false;
    };
    let mut nodes = nodes[root_index..]
        .iter()
        .filter(|node| node.mc_path_kind == McPathKind::Other);
    let Some(root) = nodes.next() else {
        return false;
    };
    let mut expected = match root.story_path_kind {
        StoryPathKind::DocumentRoot => ExpectedStoryChild::Body,
        StoryPathKind::HeaderFooterRoot => ExpectedStoryChild::Content(StoryContentKind::Block),
        StoryPathKind::FootnotesRoot => ExpectedStoryChild::Footnote,
        StoryPathKind::EndnotesRoot => ExpectedStoryChild::Endnote,
        StoryPathKind::CommentsRoot => ExpectedStoryChild::Comment,
        StoryPathKind::GlossaryRoot => ExpectedStoryChild::DocParts,
        StoryPathKind::TextBoxRoot => ExpectedStoryChild::Content(StoryContentKind::Block),
        _ => return false,
    };
    for node in nodes {
        let Some(next) = next_story_expectation(expected, node.story_path_kind) else {
            return false;
        };
        expected = next;
    }
    expected == ExpectedStoryChild::Content(StoryContentKind::Block)
}

fn text_box_path_kind(
    namespace: TextBoxNamespaceKind,
    local: &[u8],
    graphic_data_uri: Option<&str>,
    nodes: &[OpenNode],
) -> TextBoxPathKind {
    let parent = effective_text_box_parent(nodes).unwrap_or(TextBoxPathKind::Other);
    if namespace == TextBoxNamespaceKind::Word
        && local == b"pict"
        && run_child_is_schema_positioned(nodes)
    {
        return TextBoxPathKind::LegacyPicture;
    }
    if namespace == TextBoxNamespaceKind::Word
        && local == b"object"
        && run_child_is_schema_positioned(nodes)
    {
        return TextBoxPathKind::LegacyObject;
    }
    if namespace == TextBoxNamespaceKind::Word
        && local == b"drawing"
        && (run_child_is_schema_positioned(nodes) || parent == TextBoxPathKind::LegacyObject)
    {
        return TextBoxPathKind::Drawing;
    }
    if namespace == TextBoxNamespaceKind::Vml && local == b"group" {
        return match parent {
            TextBoxPathKind::LegacyPicture
            | TextBoxPathKind::LegacyObject
            | TextBoxPathKind::VmlGroup => TextBoxPathKind::VmlGroup,
            _ => TextBoxPathKind::Other,
        };
    }
    if namespace == TextBoxNamespaceKind::Vml
        && matches!(
            local,
            b"shape"
                | b"arc"
                | b"curve"
                | b"image"
                | b"line"
                | b"oval"
                | b"polyline"
                | b"rect"
                | b"roundrect"
        )
    {
        return match parent {
            TextBoxPathKind::LegacyPicture
            | TextBoxPathKind::LegacyObject
            | TextBoxPathKind::VmlGroup => TextBoxPathKind::VmlShape,
            _ => TextBoxPathKind::Other,
        };
    }
    if namespace == TextBoxNamespaceKind::Vml
        && local == b"textbox"
        && parent == TextBoxPathKind::VmlShape
    {
        return TextBoxPathKind::VmlTextBox;
    }
    if namespace == TextBoxNamespaceKind::WordprocessingDrawing
        && matches!(local, b"inline" | b"anchor")
        && parent == TextBoxPathKind::Drawing
    {
        return TextBoxPathKind::WordprocessingDrawing;
    }
    if namespace == TextBoxNamespaceKind::Drawing
        && local == b"graphic"
        && parent == TextBoxPathKind::WordprocessingDrawing
    {
        return TextBoxPathKind::Graphic;
    }
    if namespace == TextBoxNamespaceKind::Drawing
        && local == b"graphicData"
        && parent == TextBoxPathKind::Graphic
    {
        return match graphic_data_uri {
            Some(WPS_NS) => TextBoxPathKind::GraphicDataShape,
            Some(WPG_NS) => TextBoxPathKind::GraphicDataGroup,
            Some(WPC_NS) => TextBoxPathKind::GraphicDataCanvas,
            _ => TextBoxPathKind::Other,
        };
    }
    if namespace == TextBoxNamespaceKind::WordprocessingCanvas
        && local == b"wpc"
        && parent == TextBoxPathKind::GraphicDataCanvas
    {
        return TextBoxPathKind::WordprocessingCanvas;
    }
    if namespace == TextBoxNamespaceKind::WordprocessingGroup
        && local == b"wgp"
        && matches!(
            parent,
            TextBoxPathKind::GraphicDataGroup | TextBoxPathKind::WordprocessingCanvas
        )
    {
        return TextBoxPathKind::WordprocessingGroup;
    }
    if namespace == TextBoxNamespaceKind::WordprocessingGroup
        && local == b"grpSp"
        && parent == TextBoxPathKind::WordprocessingGroup
    {
        return TextBoxPathKind::WordprocessingGroup;
    }
    if namespace == TextBoxNamespaceKind::WordprocessingShape
        && local == b"wsp"
        && matches!(
            parent,
            TextBoxPathKind::GraphicDataShape
                | TextBoxPathKind::WordprocessingGroup
                | TextBoxPathKind::WordprocessingCanvas
        )
    {
        return TextBoxPathKind::WordprocessingShape;
    }
    if namespace == TextBoxNamespaceKind::WordprocessingShape
        && local == b"txbx"
        && parent == TextBoxPathKind::WordprocessingShape
    {
        return TextBoxPathKind::WordprocessingTextBox;
    }
    TextBoxPathKind::Other
}

fn effective_text_box_parent(nodes: &[OpenNode]) -> Option<TextBoxPathKind> {
    compatibility_ancestry_is_valid(nodes).then(|| {
        nodes
            .iter()
            .rev()
            .find(|node| node.mc_path_kind == McPathKind::Other)
            .map_or(TextBoxPathKind::Other, |node| node.text_box_path_kind)
    })
}

fn drawing_graphic_data_uri(
    reader: &NsReader<&[u8]>,
    element: &BytesStart<'_>,
) -> Result<Option<String>> {
    let mut uri = None;
    for attribute in element.attributes() {
        let attribute =
            attribute.map_err(|error| invalid("scan embedded XML", error.to_string()))?;
        let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
        if matches!(namespace, ResolveResult::Unbound)
            && !attribute.key.as_ref().contains(&b':')
            && local.as_ref() == b"uri"
        {
            if uri.is_some() {
                return Ok(None);
            }
            uri = Some(
                attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                    .map_err(|error| invalid("scan embedded XML", error.to_string()))?
                    .into_owned(),
            );
        }
    }
    Ok(uri)
}

fn next_story_expectation(
    expected: ExpectedStoryChild,
    actual: StoryPathKind,
) -> Option<ExpectedStoryChild> {
    match (expected, actual) {
        (ExpectedStoryChild::Body, StoryPathKind::Body)
        | (ExpectedStoryChild::Footnote, StoryPathKind::Footnote)
        | (ExpectedStoryChild::Endnote, StoryPathKind::Endnote)
        | (ExpectedStoryChild::Comment, StoryPathKind::Comment)
        | (ExpectedStoryChild::DocPartBody, StoryPathKind::DocPartBody) => {
            Some(ExpectedStoryChild::Content(StoryContentKind::Block))
        }
        (ExpectedStoryChild::DocParts, StoryPathKind::DocParts) => {
            Some(ExpectedStoryChild::DocPart)
        }
        (ExpectedStoryChild::DocPart, StoryPathKind::DocPart) => {
            Some(ExpectedStoryChild::DocPartBody)
        }
        (ExpectedStoryChild::Content(content), StoryPathKind::CustomXml) => {
            Some(ExpectedStoryChild::Content(content))
        }
        (ExpectedStoryChild::Content(content), StoryPathKind::StructuredDocumentTag) => {
            Some(ExpectedStoryChild::StructuredDocumentTagContent(content))
        }
        (
            ExpectedStoryChild::StructuredDocumentTagContent(content),
            StoryPathKind::StructuredDocumentTagContent,
        ) => Some(ExpectedStoryChild::Content(content)),
        (ExpectedStoryChild::Content(StoryContentKind::Block), StoryPathKind::Table) => {
            Some(ExpectedStoryChild::Content(StoryContentKind::TableRow))
        }
        (ExpectedStoryChild::Content(StoryContentKind::TableRow), StoryPathKind::TableRow) => {
            Some(ExpectedStoryChild::Content(StoryContentKind::TableCell))
        }
        (ExpectedStoryChild::Content(StoryContentKind::TableCell), StoryPathKind::TableCell) => {
            Some(ExpectedStoryChild::Content(StoryContentKind::Block))
        }
        _ => None,
    }
}

fn run_owner_kind(is_word: bool, local: &[u8]) -> RunOwnerKind {
    if !is_word {
        return RunOwnerKind::Other;
    }
    match local {
        b"customXml" | b"smartTag" | b"hyperlink" | b"fldSimple" | b"ins" | b"del"
        | b"moveFrom" | b"moveTo" | b"dir" | b"bdo" => RunOwnerKind::Container,
        b"sdt" => RunOwnerKind::StructuredDocumentTag,
        b"sdtContent" => RunOwnerKind::StructuredDocumentTagContent,
        _ => RunOwnerKind::Other,
    }
}

fn valid_run_owner_path(nodes: &[OpenNode]) -> bool {
    let mut needs_sdt_content = false;
    for node in nodes
        .iter()
        .filter(|node| node.mc_path_kind == McPathKind::Other)
    {
        if needs_sdt_content {
            if node.run_owner_kind != RunOwnerKind::StructuredDocumentTagContent {
                return false;
            }
            needs_sdt_content = false;
            continue;
        }
        match node.run_owner_kind {
            RunOwnerKind::Container => {}
            RunOwnerKind::StructuredDocumentTag => needs_sdt_content = true,
            RunOwnerKind::Other | RunOwnerKind::StructuredDocumentTagContent => return false,
        }
    }
    !needs_sdt_content
}

fn object_child_is_schema_positioned(nodes: &[OpenNode]) -> bool {
    if !compatibility_ancestry_is_valid(nodes) {
        return false;
    }
    let Some(mut parent) = nodes.len().checked_sub(1) else {
        return false;
    };
    while nodes[parent].mc_path_kind != McPathKind::Other {
        let Some(next) = parent.checked_sub(1) else {
            return false;
        };
        parent = next;
    }
    nodes[parent].is_object
}

fn control_child_is_schema_positioned(nodes: &[OpenNode]) -> bool {
    if !compatibility_ancestry_is_valid(nodes) {
        return false;
    }
    let Some(mut parent) = nodes.len().checked_sub(1) else {
        return false;
    };
    while nodes[parent].mc_path_kind != McPathKind::Other {
        let Some(next) = parent.checked_sub(1) else {
            return false;
        };
        parent = next;
    }
    nodes[parent].is_control_owner
}

fn record_object_child(nodes: &mut [OpenNode], relationship_id: Option<String>) -> Result<()> {
    let owner = nodes
        .iter_mut()
        .rev()
        .find(|node| node.is_object)
        .ok_or_else(|| {
            invalid(
                "scan embedded XML",
                "schema-positioned OLE object has no owning Word object".to_owned(),
            )
        })?;
    owner.object_child_count = owner.object_child_count.saturating_add(1);
    if let Some(relationship_id) = relationship_id {
        owner.object_relationship_ids.push(relationship_id);
    }
    Ok(())
}

fn record_control_child(nodes: &mut [OpenNode], relationship_id: Option<String>) -> Result<()> {
    let owner = nodes
        .iter_mut()
        .rev()
        .find(|node| node.is_control_owner)
        .ok_or_else(|| {
            invalid(
                "scan embedded XML",
                "schema-positioned Word control has no owning object or picture".to_owned(),
            )
        })?;
    owner.control_child_count = owner.control_child_count.saturating_add(1);
    if let Some(relationship_id) = relationship_id {
        if owner.control_relationship_id.is_some() {
            return Err(invalid(
                "scan embedded XML",
                format!(
                    "Word control owner at byte {} has multiple relationship ids",
                    owner.start
                ),
            ));
        }
        owner.control_relationship_id = Some(relationship_id);
    }
    Ok(())
}

fn compatibility_ancestry_is_valid(nodes: &[OpenNode]) -> bool {
    if nodes.iter().any(|node| !node.mc_rules_valid) {
        return false;
    }
    let mut index = 0usize;
    while index < nodes.len() {
        match nodes[index].mc_path_kind {
            McPathKind::Other => index += 1,
            McPathKind::AlternateContent => {
                if nodes.get(index + 1).is_none_or(|node| {
                    !matches!(node.mc_path_kind, McPathKind::Choice | McPathKind::Fallback)
                        || !node.mc_branch_valid
                }) {
                    return false;
                }
                index += 2;
            }
            McPathKind::Choice | McPathKind::Fallback => return false,
        }
    }
    true
}

fn namespace_is(namespace: &ResolveResult<'_>, expected: &[&str]) -> bool {
    matches!(namespace, ResolveResult::Bound(Namespace(uri)) if expected.iter().any(|value| *uri == value.as_bytes()))
}

fn resolved_namespace_is_ignorable(
    namespace: &ResolveResult<'_>,
    ignorable_namespaces: &BTreeSet<Vec<u8>>,
) -> bool {
    matches!(namespace, ResolveResult::Bound(Namespace(uri)) if ignorable_namespaces.contains(*uri))
}

fn local_name(name: &[u8]) -> &[u8] {
    name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
}

fn invalid(operation: &'static str, message: String) -> Error {
    Error::InvalidEmbeddedMutation { operation, message }
}

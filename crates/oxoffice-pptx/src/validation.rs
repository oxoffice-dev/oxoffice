//! Validation: the stored package checked before it opens (roots, structure, metadata, content types) and the model checked after an edit.

use super::*;

/// A package or presentation invariant that would make a saved deck unsafe.
/// New checks add variants, so a match needs a wildcard arm.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ValidationIssue {
    DuplicateShapeId {
        slide: usize,
        id: u32,
    },
    SlideIdOutOfRange {
        slide: usize,
        id: u32,
    },
    DuplicateSlideId {
        id: u32,
    },
    MissingContentTypeOverride {
        part: String,
    },
    DanglingRelationship {
        part: String,
        r_id: String,
        target: String,
    },
    UnreachableRelationshipTarget {
        part: String,
        target: String,
    },
    EmptyTextBody {
        slide: usize,
        shape: usize,
    },
    DuplicatePlaceholderIdx {
        slide: usize,
        idx: u32,
    },
    OrphanMedia {
        part: String,
    },
    CustomShowReference {
        slide_id: u32,
    },
    MissingLayoutRel {
        slide: usize,
    },
    MissingThemeRel {
        master: usize,
    },
    /// A part that belongs to one slide (its legacy or PowerPoint comments,
    /// its notes slide) related from more than one slide. PowerPoint repairs
    /// such a deck and drops the part from all but one slide.
    SharedSlidePart {
        part: String,
        /// The zero-based indices of the slides that relate to it.
        slides: Vec<usize>,
    },
    /// One violation at an explicitly supported checked XML grammar frontier.
    InvalidXmlChild {
        part: String,
        path: String,
        message: String,
    },
}

impl ValidationIssue {
    /// The snake_case name of the variant, as Python's `ValidationIssue.kind`
    /// gives it.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::DuplicateShapeId { .. } => "duplicate_shape_id",
            Self::SlideIdOutOfRange { .. } => "slide_id_out_of_range",
            Self::DuplicateSlideId { .. } => "duplicate_slide_id",
            Self::MissingContentTypeOverride { .. } => "missing_content_type_override",
            Self::DanglingRelationship { .. } => "dangling_relationship",
            Self::UnreachableRelationshipTarget { .. } => "unreachable_relationship_target",
            Self::EmptyTextBody { .. } => "empty_text_body",
            Self::DuplicatePlaceholderIdx { .. } => "duplicate_placeholder_idx",
            Self::OrphanMedia { .. } => "orphan_media",
            Self::CustomShowReference { .. } => "custom_show_reference",
            Self::MissingLayoutRel { .. } => "missing_layout_rel",
            Self::MissingThemeRel { .. } => "missing_theme_rel",
            Self::SharedSlidePart { .. } => "shared_slide_part",
            Self::InvalidXmlChild { .. } => "invalid_xml_child",
        }
    }

    /// Whether the issue is reported without blocking a save or an edit: a
    /// checked XML grammar finding, or a slide part shared by older decks
    /// (oxoffice 0.1.0 shared legacy comments after `duplicate_slide`),
    /// which PowerPoint repairs on open.
    pub(crate) fn is_report_only(&self) -> bool {
        matches!(
            self,
            Self::InvalidXmlChild { .. } | Self::SharedSlidePart { .. }
        )
    }
}

/// One sentence per issue. Slides, masters and shapes are numbered from 1, as
/// PowerPoint shows them, where the fields hold zero-based indexes.
impl std::fmt::Display for ValidationIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateShapeId { slide, id } => write!(
                f,
                "slide {}: duplicate shape ID {id}, shared by more than one shape",
                slide + 1
            ),
            Self::SlideIdOutOfRange { slide, id } => write!(
                f,
                "slide {}: slide ID {id} is outside 256 to 2147483647",
                slide + 1
            ),
            Self::DuplicateSlideId { id } => write!(f, "two slides share slide ID {id}"),
            Self::MissingContentTypeOverride { part } => {
                write!(f, "{part} has no content type override")
            }
            Self::DanglingRelationship { part, r_id, target } => write!(
                f,
                "{part}: relationship {r_id} to {target} is referenced without being defined, \
                 or defined without being referenced"
            ),
            Self::UnreachableRelationshipTarget { part, target } => {
                write!(
                    f,
                    "{part}: relationship target {target} is not in the package"
                )
            }
            Self::EmptyTextBody { slide, shape } => write!(
                f,
                "slide {}: shape {} has a text body without a paragraph",
                slide + 1,
                shape + 1
            ),
            Self::DuplicatePlaceholderIdx { slide, idx } => write!(
                f,
                "slide {}: two placeholders share placeholder idx {idx}",
                slide + 1
            ),
            Self::OrphanMedia { part } => write!(f, "{part} is not the target of any relationship"),
            Self::CustomShowReference { slide_id } => write!(
                f,
                "a custom show references relationship rId{slide_id}, which names no slide"
            ),
            Self::MissingLayoutRel { slide } => {
                write!(f, "slide {} has no slide layout relationship", slide + 1)
            }
            Self::MissingThemeRel { master } => {
                write!(f, "slide master {} has no theme relationship", master + 1)
            }
            Self::SharedSlidePart { part, slides } => write!(
                f,
                "{part} belongs to one slide but slides {} relate to it",
                slides
                    .iter()
                    .map(|slide| (slide + 1).to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::InvalidXmlChild {
                part,
                path,
                message,
            } => write!(f, "{part} {path}: {message}"),
        }
    }
}

// Stored-package validation. This route inspects
// the original XML bytes a read admits, never a reserialized model, and
// never mutates its input. It covers a finite subset only: lexical
// and character XML faults, one complete root, the p:presentation direct
// child sequence, unknown direct shape-tree children, required unqualified
// cNvPr id and name on actual physical owners, and physical table row and
// grid column equality. It is not a full schema validator.

/// The SDK 3.0.1 p:presentation direct-child sequence. Each child is a
/// singleton slot. This is the pinned generated particle, not the older API
/// remarks that also list smartTags, and it deliberately includes p:kinsoku.
const STORED_PRESENTATION_ROOT_ORDER: [&str; 14] = [
    "sldMasterIdLst",
    "notesMasterIdLst",
    "handoutMasterIdLst",
    "sldIdLst",
    "sldSz",
    "notesSz",
    "embeddedFontLst",
    "custShowLst",
    "photoAlbum",
    "custDataLst",
    "kinsoku",
    "defaultTextStyle",
    "modifyVerifier",
    "extLst",
];

const STORED_TABLE_URI: &str = "http://schemas.openxmlformats.org/drawingml/2006/table";

/// Validates the stored presentation package at `path` through its original
/// XML bytes and returns every detected issue. This reads the file only: no
/// default template is applied, no bytes variant exists, and the input file
/// is never written back. An OPC forbidden-character error becomes an existing
/// invalid-XML issue. Other file, ZIP and OPC errors propagate unchanged. The
/// checks cover a finite source subset, not full schema validation, and
/// existing live-model issues are appended after the stored ones.
pub fn validate_presentation_file<P: AsRef<Path>>(path: P) -> Result<Vec<ValidationIssue>> {
    let bytes = std::fs::read(path.as_ref()).map_err(OpcError::from)?;
    let package = match OpcPackage::from_reader(Cursor::new(&bytes)) {
        Ok(package) => package,
        Err(OpcError::InvalidXmlCharacter {
            part,
            code_point,
            line,
            column,
        }) => {
            return Ok(vec![ValidationIssue::InvalidXmlChild {
                part,
                path: "/".to_owned(),
                message: format!(
                    "holds U+{code_point:04X} at line {line}, column {column}, {}: PowerPoint cannot read that part either",
                    oxoffice_core::xml::NON_XML_CHARACTER_REFUSAL
                ),
            }]);
        }
        Err(error) => return Err(error.into()),
    };
    let mut issues = stored_package_issues(&bytes, &package);
    match Presentation::from_read_package(package) {
        Ok(presentation) => {
            for issue in presentation.validate() {
                if !issues.contains(&issue) {
                    issues.push(issue);
                }
            }
            Ok(issues)
        }
        Err(error) if issues.is_empty() => Err(error),
        Err(_) => Ok(issues),
    }
}

fn stored_package_issues(original: &[u8], package: &OpcPackage) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    let roles = stored_role_roots(package);
    let mut part_names: Vec<&String> = package.parts.keys().collect();
    part_names.sort_unstable();
    for part_name in part_names {
        if !stored_ordinary_xml_part(package, part_name) {
            continue;
        }
        issues.extend(stored_part_issues(
            part_name,
            &package.parts[part_name],
            &roles,
        ));
    }
    issues.extend(stored_metadata_issues(original));
    issues.extend(stored_content_type_issues(package));
    issues
}

/// The qualified root local name each structurally checked part must carry,
/// qualified by an actual owning role relationship plus that role's expected
/// content type, and by the officeDocument relationship plus a presentation
/// package class for the main part. Namespace vocabulary alone never makes a
/// custom part a presentation owner.
fn stored_role_roots(package: &OpcPackage) -> HashMap<String, &'static str> {
    const ROLES: [(&str, &str, &str); 6] = [
        (rel_types::SLIDE, "sld", content_types::SLIDE),
        (
            rel_types::SLIDE_LAYOUT,
            "sldLayout",
            content_types::SLIDE_LAYOUT,
        ),
        (
            rel_types::SLIDE_MASTER,
            "sldMaster",
            content_types::SLIDE_MASTER,
        ),
        (rel_types::NOTES_SLIDE, "notes", content_types::NOTES_SLIDE),
        (
            rel_types::NOTES_MASTER,
            "notesMaster",
            content_types::NOTES_MASTER,
        ),
        (
            rel_types::HANDOUT_MASTER,
            "handoutMaster",
            content_types::HANDOUT_MASTER,
        ),
    ];
    let mut roots = HashMap::new();
    let mut sources: Vec<&String> = package.part_rels.keys().collect();
    sources.sort_unstable();
    let mut collect = |source: &str, relationships: &Relationships| {
        for relationship in &relationships.items {
            if relationship_is_external(relationship) {
                continue;
            }
            let Some((expected_root, expected_type)) = ROLES
                .iter()
                .find(|(rel_type, _, _)| *rel_type == relationship.rel_type)
                .map(|(_, root, content_type)| (*root, *content_type))
            else {
                continue;
            };
            let target = OpcPackage::resolve_rel_target(source, &relationship.target);
            if package.content_types.content_type_for(&target) == Some(expected_type) {
                roots.insert(target, expected_root);
            }
        }
    };
    collect("/", &package.package_rels);
    for source in sources {
        collect(source, &package.part_rels[source]);
    }
    if let Some(main) = package.package_rels.get_by_type(rel_types::DOCUMENT)
        && !relationship_is_external(main)
    {
        let target = OpcPackage::resolve_rel_target("/", &main.target);
        if let Some(content_type) = package.content_types.content_type_for(&target)
            && PresentationPackageClass::from_content_type(content_type).is_some()
        {
            roots.insert(target, "presentation");
        }
    }
    roots
}

fn stored_ordinary_xml_part(package: &OpcPackage, part_name: &str) -> bool {
    package
        .content_types
        .content_type_for(part_name)
        .is_some_and(|content_type| content_type.to_ascii_lowercase().ends_with("xml"))
}

fn stored_part_issues(
    part: &str,
    xml: &[u8],
    roles: &HashMap<String, &'static str>,
) -> Vec<ValidationIssue> {
    let fault = |path: String, message: String| ValidationIssue::InvalidXmlChild {
        part: part.to_owned(),
        path,
        message,
    };
    let decoded = match stored_decode_xml(xml) {
        Ok(decoded) => decoded,
        Err(message) => return vec![fault("/".to_owned(), message)],
    };
    if let Some(message) = stored_lexical_fault(&decoded) {
        return vec![fault("/".to_owned(), message)];
    }
    if let Some(message) = stored_root_fault(&decoded) {
        return vec![fault("/".to_owned(), message)];
    }
    let mut names = PptXmlNames::default();
    stored_structural_issues(part, &decoded, roles, &mut names)
        .unwrap_or_else(|error| vec![("/".to_owned(), error.to_string())])
        .into_iter()
        .map(|(path, message)| fault(path, message))
        .collect()
}

/// Losslessly decodes a part to text using the same UTF-8/BOM and UTF-16
/// LE/BE recognition the OPC character scan applies. Malformed encodings are
/// rejected instead of replaced. This preserves existing opaque UTF-16 XML
/// support without claiming universal encoding support.
fn stored_decode_xml(data: &[u8]) -> std::result::Result<String, String> {
    if let Some(body) = data.strip_prefix(&[0xFF, 0xFE]) {
        return stored_decode_utf16(body, true);
    }
    if let Some(body) = data.strip_prefix(&[0xFE, 0xFF]) {
        return stored_decode_utf16(body, false);
    }
    if data.len() >= 4 && data[1] == 0 && data[3] == 0 {
        return stored_decode_utf16(data, true);
    }
    if data.len() >= 4 && data[0] == 0 && data[2] == 0 {
        return stored_decode_utf16(data, false);
    }
    let body = data.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(data);
    std::str::from_utf8(body)
        .map(str::to_owned)
        .map_err(|_| "XML part is not valid UTF-8 and has no UTF-16 signature".to_owned())
}

fn stored_decode_utf16(data: &[u8], little_endian: bool) -> std::result::Result<String, String> {
    if !data.len().is_multiple_of(2) {
        return Err("XML part ends with an odd number of UTF-16 bytes".to_owned());
    }
    let units: Vec<u16> = data
        .chunks_exact(2)
        .map(|pair| {
            if little_endian {
                u16::from_le_bytes([pair[0], pair[1]])
            } else {
                u16::from_be_bytes([pair[0], pair[1]])
            }
        })
        .collect();
    String::from_utf16(&units).map_err(|_| "XML part holds an unpaired UTF-16 surrogate".to_owned())
}

fn stored_lexical_fault(decoded: &str) -> Option<String> {
    oxoffice_core::xml::validate_strict_xml_1_0(decoded.as_bytes())
        .err()
        .map(|error| format!("{error:?}"))
}

/// Counts document roots and depth with the actual reader, because the
/// shared lexical helper checks names, attributes and references but not
/// root count or completion.
/// Counts document roots and depth with the actual reader, because the
/// shared lexical helper checks names, attributes and references but not
/// root count, completion or document position. Character-bearing events are
/// only valid inside the root, and declarations and document type
/// declarations only in the prolog.
fn stored_root_fault(decoded: &str) -> Option<String> {
    let mut reader = oxoffice_core::xml_reader::Reader::from_reader(decoded.as_bytes());
    reader.config_mut().trim_text(false);
    let mut roots = 0usize;
    let mut depth = 0usize;
    let mut root_seen = false;
    let mut prolog_done = false;
    let mut text_outside_root = false;
    loop {
        match reader.read_event() {
            Ok(XmlEvent::Start(_)) => {
                prolog_done = true;
                depth += 1;
                if depth == 1 {
                    roots += 1;
                    root_seen = true;
                }
            }
            Ok(XmlEvent::Empty(_)) => {
                prolog_done = true;
                if depth == 0 {
                    roots += 1;
                    root_seen = true;
                }
            }
            Ok(XmlEvent::End(_)) => {
                if depth == 0 {
                    return Some("XML part closes an element outside the document root".to_owned());
                }
                depth -= 1;
            }
            Ok(XmlEvent::Text(text)) => {
                if depth == 0 && !text.iter().all(u8::is_ascii_whitespace) {
                    text_outside_root = true;
                }
            }
            Ok(XmlEvent::CData(_)) if depth == 0 => {
                return Some("CDATA section outside the document root".to_owned());
            }
            Ok(XmlEvent::GeneralRef(_)) if depth == 0 => {
                return Some("entity reference outside the document root".to_owned());
            }
            Ok(XmlEvent::DocType(_)) if root_seen => {
                return Some("document type declaration outside the document prolog".to_owned());
            }
            Ok(XmlEvent::Decl(_)) if prolog_done => {
                return Some("XML declaration outside the document prolog".to_owned());
            }
            Ok(XmlEvent::Eof) => break,
            Ok(_) => {}
            Err(error) => return Some(error.to_string()),
        }
    }
    if text_outside_root {
        return Some("non-whitespace text outside the document root".to_owned());
    }
    if depth != 0 {
        return Some("XML part ends before its document root closes".to_owned());
    }
    match roots {
        1 => None,
        0 => Some("XML part has no document root".to_owned()),
        _ => Some("XML part has more than one document root".to_owned()),
    }
}

/// The shared mutable state of one stored scan, so the frame dispatch keeps
/// a bounded argument list.
struct StoredScan<'a> {
    scope: &'a mut PptXmlScope,
    names: &'a mut PptXmlNames,
    issues: &'a mut Vec<(String, String)>,
}

/// One open element frame of the iterative stored scan. Frames live in a
/// flat heap vector, so no native recursion and no recursive owned-tree
/// destruction exist on any input.
struct StoredFrame {
    kind: StoredKind,
    delta: PptXmlScopeDelta,
    route: usize,
    children: usize,
}

enum StoredKind {
    /// Consumes a subtree without structural checks: an extension island,
    /// contentPart, foreign payload or any other unchecked element.
    Opaque,
    /// p:presentation root with the finite direct-child sequence.
    PresentationRoot { last_rank: Option<usize> },
    /// A qualified role root: only cSld is structurally interesting.
    RoleRoot,
    /// p:cSld: only spTree is structurally interesting.
    CSld,
    /// spTree or grpSp: direct children are tree members. The role tree's
    /// own spTree also requires its root nvGrpSpPr cNvPr.
    TreeContainer { root_owner: bool, seen_cnvpr: bool },
    /// A physical shape member waiting for its nonvisual child.
    Owner {
        nv_local: &'static str,
        seen_cnvpr: bool,
    },
    /// The nonvisual-property child of an owner.
    NonVisual { seen_cnvpr: bool },
    /// a:graphic waiting for a:graphicData.
    Graphic,
    /// a:graphicData: table content only under the table URI.
    GraphicData { table: bool },
    /// a:tbl with the direct grid column count of its own tblGrid.
    Table { columns: Option<usize> },
    /// a:tblGrid counting direct gridCol children.
    TableGrid { columns: usize },
    /// a:tr counting direct tc children.
    TableRow { cells: usize },
    /// mc:AlternateContent at a tree-member position.
    Alternate,
    /// mc:Choice or mc:Fallback whose members inherit the tree context.
    Branch,
    /// The role tree root's own nvGrpSpPr.
    RootNonVisual { seen_cnvpr: bool },
}

/// Iterative finite structural scan of one qualified role part. The part
/// must be an actual role owner per the approved contract; every other XML
/// source already received its lexical and root checks and stays opaque.
fn stored_structural_issues(
    part: &str,
    decoded: &str,
    roles: &HashMap<String, &'static str>,
    names: &mut PptXmlNames,
) -> Result<Vec<(String, String)>> {
    let Some(expected_root) = roles.get(part).copied() else {
        return Ok(Vec::new());
    };
    let mut reader = oxoffice_core::xml_reader::Reader::from_reader(decoded.as_bytes());
    reader.config_mut().trim_text(false);
    let mut scope = PptXmlScope::default();
    let mut issues = Vec::new();
    let root = loop {
        match reader
            .read_event()
            .map_err(|error| invalid_shape_mutation("stored validation", error.to_string()))?
        {
            XmlEvent::Start(element) => break element,
            XmlEvent::Empty(_) | XmlEvent::Eof => return Ok(issues),
            _ => {}
        }
    };
    let delta = scope.enter(&root, names)?;
    let (uri, local) = scope.name(root.name().as_ref(), false, names)?;
    if names.values[uri] != P_NS || names.values[local] != expected_root {
        scope.leave(delta);
        return Ok(issues);
    }
    let root_route = names.route(0, 0, uri, local);
    let root_kind = if names.values[local] == "presentation" {
        StoredKind::PresentationRoot { last_rank: None }
    } else {
        StoredKind::RoleRoot
    };
    let mut stack: Vec<StoredFrame> = vec![StoredFrame {
        kind: root_kind,
        delta,
        route: root_route,
        children: 0,
    }];
    loop {
        let event = reader
            .read_event()
            .map_err(|error| invalid_shape_mutation("stored validation", error.to_string()))?;
        let mut scan = StoredScan {
            scope: &mut scope,
            names,
            issues: &mut issues,
        };
        match event {
            XmlEvent::Start(element) => {
                stored_start_element(element, &mut stack, &mut scan)?;
            }
            XmlEvent::Empty(element) => {
                stored_start_element(element, &mut stack, &mut scan)?;
                stored_end_element(&mut stack, &mut scan)?;
            }
            XmlEvent::End(_) => {
                stored_end_element(&mut stack, &mut scan)?;
            }
            XmlEvent::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() {
        issues.push((
            "/".to_owned(),
            "XML part ends before its root element closes".to_owned(),
        ));
    }
    Ok(issues)
}

fn stored_start_element(
    element: XmlBytesStart<'_>,
    stack: &mut Vec<StoredFrame>,
    scan: &mut StoredScan<'_>,
) -> Result<()> {
    let delta = scan.scope.enter(&element, scan.names)?;
    let Some(top) = stack.last_mut() else {
        scan.scope.leave(delta);
        return Err(invalid_shape_mutation(
            "stored validation",
            "element outside the document root",
        ));
    };
    if matches!(top.kind, StoredKind::Opaque) {
        stack.push(StoredFrame {
            kind: StoredKind::Opaque,
            delta,
            route: 0,
            children: 0,
        });
        return Ok(());
    }
    let (uri, local) = scan
        .scope
        .name(element.name().as_ref(), false, scan.names)?;
    let ignorable = scan.scope.ignorable.contains(&uri);
    let route = scan.names.route(top.route, top.children, uri, local);
    top.children += 1;
    let child_kind =
        stored_child_kind(&mut top.kind, uri, local, ignorable, route, &element, scan)?;
    stack.push(StoredFrame {
        kind: child_kind,
        delta,
        route,
        children: 0,
    });
    Ok(())
}

fn stored_end_element(stack: &mut Vec<StoredFrame>, scan: &mut StoredScan<'_>) -> Result<()> {
    let Some(frame) = stack.pop() else {
        return Err(invalid_shape_mutation(
            "stored validation",
            "XML part closes an element outside the document root",
        ));
    };
    scan.scope.leave(frame.delta);
    match frame.kind {
        StoredKind::Owner {
            seen_cnvpr: false, ..
        } => {
            scan.issues.push((
                scan.names.path(frame.route),
                "required cNvPr is absent".to_owned(),
            ));
        }
        StoredKind::NonVisual { seen_cnvpr } => {
            if let Some(StoredKind::Owner {
                seen_cnvpr: flag, ..
            }) = stack.last_mut().map(|owner| &mut owner.kind)
            {
                *flag = seen_cnvpr;
            }
        }
        StoredKind::TreeContainer {
            root_owner: false,
            seen_cnvpr: false,
        } => {
            scan.issues.push((
                scan.names.path(frame.route),
                "required cNvPr is absent".to_owned(),
            ));
        }
        StoredKind::RootNonVisual { seen_cnvpr } => {
            if let Some(StoredKind::TreeContainer {
                seen_cnvpr: flag, ..
            }) = stack.last_mut().map(|owner| &mut owner.kind)
            {
                *flag = seen_cnvpr;
            }
            if !seen_cnvpr {
                scan.issues.push((
                    scan.names.path(frame.route),
                    "required cNvPr is absent".to_owned(),
                ));
            }
        }
        StoredKind::TableGrid { columns } => {
            if let Some(StoredKind::Table { columns: target }) =
                stack.last_mut().map(|table| &mut table.kind)
            {
                *target = Some(columns);
            }
        }
        StoredKind::TableRow { cells } => {
            if let Some(StoredKind::Table { columns }) =
                stack.last_mut().map(|table| &mut table.kind)
                && let Some(columns) = *columns
                && cells != columns
            {
                scan.issues.push((
                    scan.names.path(frame.route),
                    format!("table row has {cells} cells but its grid has {columns} columns"),
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

/// Resolves what frame an opening child of `top` needs, reporting the finite
/// membership, sequence, nonvisual and row faults on the way.
fn stored_child_kind(
    top: &mut StoredKind,
    uri: usize,
    local: usize,
    ignorable: bool,
    route: usize,
    element: &XmlBytesStart<'_>,
    scan: &mut StoredScan<'_>,
) -> Result<StoredKind> {
    let uri = scan.names.values[uri].clone();
    let local = scan.names.values[local].clone();
    match top {
        StoredKind::Opaque => unreachable!("opaque frames resolve no children"),
        StoredKind::PresentationRoot { last_rank } => {
            if uri == P_NS {
                match STORED_PRESENTATION_ROOT_ORDER
                    .iter()
                    .position(|name| *name == local)
                {
                    Some(rank) => {
                        if last_rank.is_some_and(|last| rank <= last) {
                            scan.issues.push((
                                scan.names.path(route),
                                "finite XML child order or multiplicity is invalid".to_owned(),
                            ));
                        }
                        if last_rank.is_none_or(|last| rank > last) {
                            *last_rank = Some(rank);
                        }
                    }
                    None => scan.issues.push(stored_unknown_child(route, scan.names)),
                }
            } else if !ignorable {
                scan.issues.push(stored_unknown_child(route, scan.names));
            }
            Ok(StoredKind::Opaque)
        }
        StoredKind::RoleRoot => Ok(if uri == P_NS && local == "cSld" {
            StoredKind::CSld
        } else {
            StoredKind::Opaque
        }),
        StoredKind::CSld => Ok(if uri == P_NS && local == "spTree" {
            StoredKind::TreeContainer {
                root_owner: true,
                seen_cnvpr: false,
            }
        } else {
            StoredKind::Opaque
        }),
        StoredKind::TreeContainer { .. } => {
            Ok(match uri.as_str() {
                P_NS if local == "nvGrpSpPr" => StoredKind::RootNonVisual { seen_cnvpr: false },
                P_NS => {
                    if local == "grpSp" {
                        StoredKind::TreeContainer {
                            root_owner: false,
                            seen_cnvpr: false,
                        }
                    } else if matches!(local.as_str(), "sp" | "pic" | "cxnSp" | "graphicFrame") {
                        StoredKind::Owner {
                            nv_local: ppt_shape_nonvisual_parent(&local),
                            seen_cnvpr: false,
                        }
                    } else if stored_tree_membership(&local) {
                        // nvGrpSpPr, grpSpPr, extLst and contentPart are
                        // accepted members.
                        StoredKind::Opaque
                    } else {
                        // An ignorable namespace never excuses an unknown
                        // PresentationML child.
                        scan.issues.push(stored_unknown_child(route, scan.names));
                        StoredKind::Opaque
                    }
                }
                PPT_MC_NS if local == "AlternateContent" => StoredKind::Alternate,
                _ if ignorable => StoredKind::Opaque,
                _ => {
                    scan.issues.push(stored_unknown_child(route, scan.names));
                    StoredKind::Opaque
                }
            })
        }
        StoredKind::Owner { nv_local, .. } => {
            let nv_local = *nv_local;
            Ok(if uri == P_NS && local == nv_local {
                StoredKind::NonVisual { seen_cnvpr: false }
            } else if nv_local == "nvGraphicFramePr" && uri == A_NS && local == "graphic" {
                StoredKind::Graphic
            } else {
                StoredKind::Opaque
            })
        }
        StoredKind::NonVisual { seen_cnvpr } => {
            if uri == P_NS && local == "cNvPr" {
                *seen_cnvpr = true;
                stored_cnvpr_attr_issues(route, element, scan)?;
            }
            Ok(StoredKind::Opaque)
        }
        StoredKind::RootNonVisual { seen_cnvpr } => {
            if uri == P_NS && local == "cNvPr" {
                *seen_cnvpr = true;
                stored_cnvpr_attr_issues(route, element, scan)?;
            }
            Ok(StoredKind::Opaque)
        }
        StoredKind::Graphic => Ok(if uri == A_NS && local == "graphicData" {
            StoredKind::GraphicData {
                table: stored_unqualified_attr(element, scan, "uri")?
                    .is_some_and(|value| value == STORED_TABLE_URI),
            }
        } else {
            StoredKind::Opaque
        }),
        StoredKind::GraphicData { table } => Ok(if *table && uri == A_NS && local == "tbl" {
            StoredKind::Table { columns: None }
        } else {
            StoredKind::Opaque
        }),
        StoredKind::Table { .. } => Ok(match (uri == A_NS, local.as_str()) {
            (true, "tblGrid") => StoredKind::TableGrid { columns: 0 },
            (true, "tr") => StoredKind::TableRow { cells: 0 },
            _ => StoredKind::Opaque,
        }),
        StoredKind::TableGrid { columns } => {
            if uri == A_NS && local == "gridCol" {
                *columns += 1;
            }
            Ok(StoredKind::Opaque)
        }
        StoredKind::TableRow { cells } => {
            if uri == A_NS && local == "tc" {
                *cells += 1;
            }
            Ok(StoredKind::Opaque)
        }
        StoredKind::Alternate => Ok(
            if uri == PPT_MC_NS && matches!(local.as_str(), "Choice" | "Fallback") {
                StoredKind::Branch
            } else {
                StoredKind::Opaque
            },
        ),
        StoredKind::Branch => Ok(match uri.as_str() {
            P_NS => {
                // Members inherit the tree context: the same finite rule as
                // the enclosing tree container, so a supported chart Choice
                // cannot hide a missing nonvisual name.
                if local == "grpSp" {
                    StoredKind::TreeContainer {
                        root_owner: false,
                        seen_cnvpr: false,
                    }
                } else if matches!(local.as_str(), "sp" | "pic" | "cxnSp" | "graphicFrame") {
                    StoredKind::Owner {
                        nv_local: ppt_shape_nonvisual_parent(&local),
                        seen_cnvpr: false,
                    }
                } else if stored_tree_membership(&local) {
                    StoredKind::Opaque
                } else {
                    scan.issues.push(stored_unknown_child(route, scan.names));
                    StoredKind::Opaque
                }
            }
            PPT_MC_NS if local == "AlternateContent" => StoredKind::Alternate,
            // Foreign branch payload stays opaque and is never treated as
            // PresentationML owners by local-name resemblance.
            _ => StoredKind::Opaque,
        }),
    }
}

/// The finite member set shared by spTree, grpSp and qualified immediate
/// compatibility-branch members. Membership comes from the existing
/// child-order helper plus the stored-only p:contentPart allowance.
fn stored_tree_membership(local: &str) -> bool {
    ppt_xml_child_order(P_NS, b"spTree", local.as_bytes()).is_some() || local == "contentPart"
}

/// The unknown-direct-child issue for one resolved child route.
fn stored_unknown_child(route: usize, names: &PptXmlNames) -> (String, String) {
    (
        names.path(route),
        "unknown or misplaced direct XML child".to_owned(),
    )
}

/// Reports the required unqualified cNvPr id and name attributes at their
/// own element route, mirroring the approved nonvisual-owner frontier.
fn stored_cnvpr_attr_issues(
    route: usize,
    element: &XmlBytesStart<'_>,
    scan: &mut StoredScan<'_>,
) -> Result<()> {
    let id = stored_unqualified_attr(element, scan, "id")?;
    let name = stored_unqualified_attr(element, scan, "name")?;
    if id.is_none() {
        scan.issues.push((
            scan.names.path(route),
            "required cNvPr id attribute is absent".to_owned(),
        ));
    }
    if name.is_none() {
        scan.issues.push((
            scan.names.path(route),
            "required cNvPr name attribute is absent".to_owned(),
        ));
    }
    Ok(())
}

/// The decoded value of one unqualified attribute of this element, resolved
/// through the existing namespace scope. Namespaced lookalikes never match.
fn stored_unqualified_attr(
    element: &XmlBytesStart<'_>,
    scan: &mut StoredScan<'_>,
    local: &str,
) -> Result<Option<String>> {
    let expected = (scan.names.id(""), scan.names.id(local));
    let mut value = None;
    for attribute in element.attributes() {
        let attribute = attribute
            .map_err(|error| invalid_shape_mutation("stored validation", error.to_string()))?;
        let raw = attribute.key.as_ref();
        if raw == b"xmlns" || raw.starts_with(b"xmlns:") {
            continue;
        }
        if scan.scope.name(raw, true, scan.names)? == expected {
            value = Some(
                attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                    .map_err(|error| {
                        invalid_shape_mutation("stored validation", error.to_string())
                    })?
                    .into_owned(),
            );
        }
    }
    Ok(value)
}

/// Recovers the original [Content_Types].xml and relationship bytes from a
/// read-only archive over the retained original file bytes. Ordinary parts
/// are already carried unmodified in the package; only metadata is not.
fn stored_metadata_issues(original: &[u8]) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    let Ok(mut archive) = zip::ZipArchive::new(Cursor::new(original)) else {
        return issues;
    };
    let mut metadata: Vec<(usize, String)> = Vec::new();
    for index in 0..archive.len() {
        let Ok(entry) = archive.by_index(index) else {
            continue;
        };
        let Ok(raw_name) = entry.name() else {
            continue;
        };
        // Original metadata is the root content-types identity under the
        // loader's normalization and case policy, plus relationship parts.
        // A name suffix alone must not promote a binary or nested ordinary
        // part to metadata.
        let normalized = OpcPackage::resolve_rel_target("/", &raw_name);
        let is_content_types = normalized.eq_ignore_ascii_case("/[Content_Types].xml");
        let is_relationships = normalized.to_ascii_lowercase().ends_with(".rels");
        if is_content_types || is_relationships {
            metadata.push((index, raw_name.to_string()));
        }
    }
    metadata.sort_by(|(_, a), (_, b)| a.cmp(b));
    for (index, name) in metadata {
        let Ok(mut entry) = archive.by_index(index) else {
            continue;
        };
        let mut xml = Vec::new();
        if std::io::Read::read_to_end(&mut entry, &mut xml).is_err() {
            continue;
        }
        if let Some(message) = stored_metadata_fault(&xml) {
            let part = format!("/{}", name.strip_prefix('/').unwrap_or(&name));
            issues.push(ValidationIssue::InvalidXmlChild {
                part,
                path: "/".to_owned(),
                message,
            });
        }
    }
    issues
}

fn stored_metadata_fault(xml: &[u8]) -> Option<String> {
    let decoded = match stored_decode_xml(xml) {
        Ok(decoded) => decoded,
        Err(message) => return Some(message),
    };
    stored_lexical_fault(&decoded).or_else(|| stored_root_fault(&decoded))
}

/// Requires the matching existing content-type constant for every internal
/// relationship of a PowerPoint role, resolved through its actual owning
/// part. A missing, fallback or wrong type is reported for the target part.
fn stored_content_type_issues(package: &OpcPackage) -> Vec<ValidationIssue> {
    const ROLES: [(&str, &str); 6] = [
        (rel_types::SLIDE, content_types::SLIDE),
        (rel_types::SLIDE_LAYOUT, content_types::SLIDE_LAYOUT),
        (rel_types::SLIDE_MASTER, content_types::SLIDE_MASTER),
        (rel_types::NOTES_SLIDE, content_types::NOTES_SLIDE),
        (rel_types::NOTES_MASTER, content_types::NOTES_MASTER),
        (rel_types::HANDOUT_MASTER, content_types::HANDOUT_MASTER),
    ];
    let mut issues = Vec::new();
    let mut sources: Vec<&String> = package.part_rels.keys().collect();
    sources.sort_unstable();
    let mut check = |source: &str, relationships: &Relationships| {
        for relationship in &relationships.items {
            if relationship_is_external(relationship) {
                continue;
            }
            let Some(expected) = ROLES
                .iter()
                .find(|(rel_type, _)| *rel_type == relationship.rel_type)
                .map(|(_, content_type)| *content_type)
            else {
                continue;
            };
            let target = OpcPackage::resolve_rel_target(source, &relationship.target);
            if package.content_types.content_type_for(&target) != Some(expected)
                && !issues.contains(&ValidationIssue::MissingContentTypeOverride {
                    part: target.clone(),
                })
            {
                issues.push(ValidationIssue::MissingContentTypeOverride { part: target });
            }
        }
    };
    check("/", &package.package_rels);
    for source in sources {
        check(source, &package.part_rels[source]);
    }
    issues
}

impl Presentation {
    /// Returns every detected package and PresentationML invariant violation.
    pub fn validate(&self) -> Vec<ValidationIssue> {
        let mut duplicate_shape_ids = Vec::new();
        let mut out_of_range_slide_ids = Vec::new();
        let mut duplicate_slide_ids = Vec::new();
        let mut missing_content_types = Vec::new();
        let mut dangling_relationships = Vec::new();
        let mut unreachable_targets = Vec::new();
        let mut empty_text_bodies = Vec::new();
        let mut duplicate_placeholder_indices = Vec::new();
        let mut orphan_media = Vec::new();
        let mut custom_show_references = Vec::new();
        let mut missing_layout_relationships = Vec::new();
        let mut missing_theme_relationships = Vec::new();

        for (slide_index, record) in self.slides.iter().enumerate() {
            let mut shape_ids = record
                .slide
                .common_slide_data
                .shape_tree
                .non_visual_id()
                .into_iter()
                .collect();
            let mut placeholder_indices = HashSet::new();
            let mut shape_index = 0usize;
            validate_shape_children(
                slide_index,
                &record.slide.common_slide_data.shape_tree.children,
                &mut shape_index,
                &mut shape_ids,
                &mut placeholder_indices,
                &mut duplicate_shape_ids,
                &mut empty_text_bodies,
                &mut duplicate_placeholder_indices,
            );
        }

        let mut slide_ids = HashSet::new();
        for (slide_index, slide) in self.presentation.slide_ids.iter().enumerate() {
            if !(256..=2_147_483_647).contains(&slide.id) {
                out_of_range_slide_ids.push(ValidationIssue::SlideIdOutOfRange {
                    slide: slide_index,
                    id: slide.id,
                });
            }
            if !slide_ids.insert(slide.id) {
                duplicate_slide_ids.push(ValidationIssue::DuplicateSlideId { id: slide.id });
            }
        }

        let mut part_names = self.package.parts.keys().collect::<Vec<_>>();
        part_names.sort_unstable();
        for part_name in part_names {
            if !part_has_content_type(&self.package, part_name) {
                missing_content_types.push(ValidationIssue::MissingContentTypeOverride {
                    part: part_name.clone(),
                });
            }
        }

        let mut incoming_targets = HashSet::new();
        validate_relationship_scope(
            &self.package,
            "/",
            None,
            &self.package.package_rels,
            &mut incoming_targets,
            &mut dangling_relationships,
            &mut unreachable_targets,
        );
        let mut relationship_sources = self
            .package
            .parts
            .keys()
            .chain(self.package.part_rels.keys())
            .map(String::as_str)
            .collect::<Vec<_>>();
        relationship_sources.sort_unstable();
        relationship_sources.dedup();
        let empty_relationships = Relationships::new();
        for source_part in relationship_sources {
            let relationships = self
                .package
                .get_part_rels(source_part)
                .unwrap_or(&empty_relationships);
            let source_xml = self.current_part_xml(source_part);
            validate_relationship_scope(
                &self.package,
                source_part,
                source_xml.as_deref(),
                relationships,
                &mut incoming_targets,
                &mut dangling_relationships,
                &mut unreachable_targets,
            );
        }

        let mut media_parts = self
            .package
            .parts
            .keys()
            .filter(|part| part.to_ascii_lowercase().starts_with("/ppt/media/"))
            .collect::<Vec<_>>();
        media_parts.sort_unstable();
        for part in media_parts {
            if !incoming_targets.contains(&part.to_ascii_lowercase()) {
                orphan_media.push(ValidationIssue::OrphanMedia { part: part.clone() });
            }
        }

        let presentation_relationships = self.package.get_part_rels(&self.presentation_part);
        let slide_relationship_ids = self
            .presentation
            .slide_ids
            .iter()
            .map(|slide| slide.relationship_id.as_str())
            .collect::<HashSet<_>>();
        if let Some(xml) = self.current_part_xml(&self.presentation_part)
            && let Ok(references) = custom_show_relationship_ids(&xml)
        {
            for relationship_id in references {
                if !slide_relationship_ids.contains(relationship_id.as_str()) {
                    custom_show_references.push(ValidationIssue::CustomShowReference {
                        slide_id: numeric_relationship_id(&relationship_id),
                    });
                }
            }
        }

        for (slide_index, slide) in self.slides.iter().enumerate() {
            let count = self
                .package
                .get_part_rels(&slide.part_name)
                .map(|relationships| {
                    relationships
                        .items
                        .iter()
                        .filter(|relationship| {
                            relationship.rel_type == rel_types::SLIDE_LAYOUT
                                && !relationship_is_external(relationship)
                        })
                        .count()
                })
                .unwrap_or_default();
            if count != 1 {
                missing_layout_relationships
                    .push(ValidationIssue::MissingLayoutRel { slide: slide_index });
            }
        }

        // Comments and notes are per slide: no two slides relate to one part.
        // Keyed by the lowercase part name; the first spelling is reported.
        let mut owners_by_part: HashMap<String, (String, Vec<usize>)> = HashMap::new();
        for (slide_index, slide) in self.slides.iter().enumerate() {
            let Some(relationships) = self.package.get_part_rels(&slide.part_name) else {
                continue;
            };
            for relationship in &relationships.items {
                if relationship_is_external(relationship)
                    || ![
                        rel_types::COMMENTS,
                        rel_types::POWERPOINT_COMMENTS,
                        rel_types::NOTES_SLIDE,
                    ]
                    .contains(&relationship.rel_type.as_str())
                {
                    continue;
                }
                let part = OpcPackage::resolve_rel_target(&slide.part_name, &relationship.target);
                let (_, slides) = owners_by_part
                    .entry(part.to_ascii_lowercase())
                    .or_insert_with(|| (part.to_string(), Vec::new()));
                if !slides.contains(&slide_index) {
                    slides.push(slide_index);
                }
            }
        }
        let mut shared_slide_parts = owners_by_part
            .into_values()
            .filter(|(_, slides)| slides.len() > 1)
            .map(|(part, slides)| ValidationIssue::SharedSlidePart { part, slides })
            .collect::<Vec<_>>();
        shared_slide_parts.sort_by_key(ToString::to_string);

        for (master_index, master) in self.presentation.slide_master_ids.iter().enumerate() {
            let Some(relationship) = presentation_relationships
                .and_then(|relationships| relationships.get_by_id(&master.relationship_id))
            else {
                missing_theme_relationships.push(ValidationIssue::MissingThemeRel {
                    master: master_index,
                });
                continue;
            };
            let master_part =
                OpcPackage::resolve_rel_target(&self.presentation_part, &relationship.target);
            let has_theme = self
                .package
                .get_part_rels(&master_part)
                .is_some_and(|relationships| {
                    relationships.items.iter().any(|relationship| {
                        relationship.rel_type == rel_types::THEME
                            && !relationship_is_external(relationship)
                    })
                });
            if !has_theme {
                missing_theme_relationships.push(ValidationIssue::MissingThemeRel {
                    master: master_index,
                });
            }
        }

        let mut invalid_xml = Vec::new();
        let owners = (0..self.slides.len())
            .map(ShapeOwner::Slide)
            .chain((0..self.layouts.len()).map(ShapeOwner::Layout))
            .chain((0..self.masters.len()).map(ShapeOwner::Master));
        for owner in owners {
            let Ok(part) = self.owner_part_name(owner) else {
                continue;
            };
            let mut names = PptXmlNames::default();
            let Some(xml) = self.current_part_xml(part) else {
                continue;
            };
            match ppt_xml_frontiers(&xml, &mut names) {
                Ok(scan) => invalid_xml.extend(scan.islands.iter().map(|island| {
                    ValidationIssue::InvalidXmlChild {
                        part: part.to_owned(),
                        path: names.path(island.route),
                        message: island.message.to_owned(),
                    }
                })),
                Err(error) => invalid_xml.push(ValidationIssue::InvalidXmlChild {
                    part: part.to_owned(),
                    path: "/".to_owned(),
                    message: error.to_string(),
                }),
            }
        }
        duplicate_shape_ids
            .into_iter()
            .chain(out_of_range_slide_ids)
            .chain(duplicate_slide_ids)
            .chain(missing_content_types)
            .chain(dangling_relationships)
            .chain(unreachable_targets)
            .chain(empty_text_bodies)
            .chain(duplicate_placeholder_indices)
            .chain(orphan_media)
            .chain(custom_show_references)
            .chain(missing_layout_relationships)
            .chain(missing_theme_relationships)
            .chain(shared_slide_parts)
            .chain(invalid_xml)
            .collect()
    }

    fn current_part_xml(&self, part_name: &str) -> Option<Vec<u8>> {
        if part_name.eq_ignore_ascii_case(&self.presentation_part) {
            return self.presentation.to_xml().ok();
        }
        if let Some(slide) = self
            .slides
            .iter()
            .find(|slide| slide.part_name.eq_ignore_ascii_case(part_name))
        {
            return slide.slide.to_xml().ok();
        }
        if let Some(notes) = self
            .slides
            .iter()
            .filter_map(|slide| slide.notes.as_ref())
            .find(|notes| notes.part_name.eq_ignore_ascii_case(part_name))
        {
            return notes.notes.to_xml().ok();
        }
        if let Some(record) = self
            .masters
            .iter()
            .find(|record| record.dirty && record.part_name.eq_ignore_ascii_case(part_name))
        {
            return record.master.as_ref().ok()?.to_xml().ok();
        }
        if let Some(record) = self
            .layouts
            .iter()
            .find(|record| record.dirty && record.part_name.eq_ignore_ascii_case(part_name))
        {
            return record.layout.to_xml().ok();
        }
        if let Some(record) = self
            .themes
            .iter()
            .find(|record| record.dirty && record.part_name.eq_ignore_ascii_case(part_name))
        {
            return record.theme.to_xml().ok();
        }
        self.package.get_part(part_name).map(<[u8]>::to_vec)
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_shape_children(
    slide_index: usize,
    children: &[ShapeTreeChild],
    shape_index: &mut usize,
    shape_ids: &mut HashSet<u32>,
    placeholder_indices: &mut HashSet<u32>,
    duplicate_shape_ids: &mut Vec<ValidationIssue>,
    empty_text_bodies: &mut Vec<ValidationIssue>,
    duplicate_placeholder_indices: &mut Vec<ValidationIssue>,
) {
    let mut pending = children.iter().rev().collect::<Vec<_>>();
    while let Some(child) = pending.pop() {
        let current_shape_index = *shape_index;
        *shape_index += 1;
        if let Some(id) = child.non_visual_id()
            && !shape_ids.insert(id)
        {
            duplicate_shape_ids.push(ValidationIssue::DuplicateShapeId {
                slide: slide_index,
                id,
            });
        }
        let placeholder = match child {
            ShapeTreeChild::Shape(shape) => {
                if shape
                    .text_body
                    .as_ref()
                    .is_some_and(|body| body.paragraph_count() == 0)
                {
                    empty_text_bodies.push(ValidationIssue::EmptyTextBody {
                        slide: slide_index,
                        shape: current_shape_index,
                    });
                }
                shape.placeholder.as_ref()
            }
            ShapeTreeChild::Picture(picture) => picture.placeholder.as_ref(),
            _ => None,
        };
        if let Some(idx) = placeholder
            .and_then(|placeholder| placeholder.idx)
            .filter(|idx| *idx != u32::MAX)
            && !placeholder_indices.insert(idx)
        {
            duplicate_placeholder_indices.push(ValidationIssue::DuplicatePlaceholderIdx {
                slide: slide_index,
                idx,
            });
        }
        match child {
            ShapeTreeChild::GroupShape(group) => {
                pending.extend(group.children.iter().rev());
            }
            ShapeTreeChild::AlternateContent(alternate) => {
                if let Some(fallback) = alternate.selected_fallback() {
                    pending.extend(fallback.iter().rev());
                }
            }
            _ => {}
        }
    }
}

fn validate_relationship_scope(
    package: &OpcPackage,
    source_part: &str,
    source_xml: Option<&[u8]>,
    relationships: &Relationships,
    incoming_targets: &mut HashSet<String>,
    dangling_relationships: &mut Vec<ValidationIssue>,
    unreachable_targets: &mut Vec<ValidationIssue>,
) {
    let referenced = source_xml.and_then(|xml| relationship_ids(xml).ok());
    let has_relationship_namespace = source_xml.is_some_and(|xml| {
        xml.windows(oxoffice_pptx_oxml::namespace::R_NS.len())
            .any(|window| window == oxoffice_pptx_oxml::namespace::R_NS.as_bytes())
    });
    if let Some(referenced) = &referenced {
        let mut reported = HashSet::new();
        for relationship_id in referenced {
            if relationships.get_by_id(relationship_id).is_none()
                && reported.insert(relationship_id)
            {
                dangling_relationships.push(ValidationIssue::DanglingRelationship {
                    part: source_part.to_owned(),
                    r_id: relationship_id.clone(),
                    target: relationship_id.clone(),
                });
            }
        }
    }
    for relationship in &relationships.items {
        if let Some(referenced) = &referenced
            && has_relationship_namespace
            && relationship_requires_xml_reference(&relationship.rel_type)
            && !referenced.iter().any(|id| id == &relationship.id)
        {
            dangling_relationships.push(ValidationIssue::DanglingRelationship {
                part: source_part.to_owned(),
                r_id: relationship.id.clone(),
                target: relationship.target.clone(),
            });
        }
        if relationship_is_external(relationship) {
            continue;
        }
        let target = OpcPackage::resolve_rel_target(source_part, &relationship.target);
        incoming_targets.insert(target.to_ascii_lowercase());
        if !package.contains_part(&target) {
            unreachable_targets.push(ValidationIssue::UnreachableRelationshipTarget {
                part: source_part.to_owned(),
                target,
            });
        }
    }
}

pub(super) fn relationship_requires_xml_reference(relationship_type: &str) -> bool {
    matches!(
        relationship_type,
        rel_types::IMAGE | rel_types::HYPERLINK | rel_types::CHART
    ) || relationship_type.ends_with("/audio")
        || relationship_type.ends_with("/video")
        || relationship_type.ends_with("/oleObject")
        || relationship_type.ends_with("/diagramData")
}

fn part_has_content_type(package: &OpcPackage, part_name: &str) -> bool {
    package.content_types.content_type_for(part_name).is_some()
        || part_name.rsplit_once('.').is_some_and(|(_, extension)| {
            package
                .content_types
                .defaults
                .contains_key(&extension.to_ascii_lowercase())
        })
}

fn numeric_relationship_id(relationship_id: &str) -> u32 {
    relationship_id
        .strip_prefix("rId")
        .and_then(|value| value.parse().ok())
        .unwrap_or_default()
}

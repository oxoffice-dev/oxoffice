//! Raw XML of owners: reading and replacing the XML of a shape, table, cell, text body, paragraph, run, slide, layout or master, with the frontier, namespace and relationship checks that guard it.

use super::*;

// Finite physical owner cases for the nine approved XML operations.
#[derive(Clone, Copy)]
pub(super) enum PptXmlOwner<'a> {
    Part(ShapeOwner),
    Shape(ShapeOwner, &'a [usize]),
    Table(ShapeOwner, &'a [usize]),
    Cell(ShapeOwner, &'a [usize], usize, usize),
    TextBody(ShapeOwner, &'a [usize], Option<(usize, usize)>),
    Paragraph(ShapeOwner, &'a [usize], Option<(usize, usize)>, usize),
    Run(
        ShapeOwner,
        &'a [usize],
        Option<(usize, usize)>,
        usize,
        usize,
    ),
}

impl PptXmlOwner<'_> {
    fn part(self) -> ShapeOwner {
        match self {
            Self::Part(part)
            | Self::Shape(part, _)
            | Self::Table(part, _)
            | Self::Cell(part, _, _, _)
            | Self::TextBody(part, _, _)
            | Self::Paragraph(part, _, _, _)
            | Self::Run(part, _, _, _, _) => part,
        }
    }

    fn root(self, presentation: &Presentation) -> Result<(&'static str, &'static [u8])> {
        Ok(match self {
            Self::Part(ShapeOwner::Slide(_)) => (P_NS, b"sld"),
            Self::Part(ShapeOwner::Layout(_)) => (P_NS, b"sldLayout"),
            Self::Part(ShapeOwner::Master(_)) => (P_NS, b"sldMaster"),
            Self::Table(_, _) => (A_NS, b"tbl"),
            Self::Cell(_, _, _, _) => (A_NS, b"tc"),
            Self::TextBody(_, _, cell) => (if cell.is_some() { A_NS } else { P_NS }, b"txBody"),
            Self::Paragraph(_, _, _, _) => (A_NS, b"p"),
            Self::Run(_, _, _, _, _) => (A_NS, b"r"),
            Self::Shape(part, path) => (
                P_NS,
                match presentation.part_shape_at(part, path)?.child {
                    ShapeTreeChild::Shape(_) => b"sp",
                    ShapeTreeChild::Picture(_) => b"pic",
                    ShapeTreeChild::GraphicFrame(_) => b"graphicFrame",
                    ShapeTreeChild::GroupShape(_) => b"grpSp",
                    ShapeTreeChild::Connector(_) => b"cxnSp",
                    ShapeTreeChild::AlternateContent(_) => {
                        return Err(invalid_shape_mutation(
                            "replace XML",
                            "raw alternate shape owner is read only",
                        ));
                    }
                },
            ),
        })
    }
}

fn ppt_xml_close_namespaces(xml: &[u8]) -> Result<Vec<u8>> {
    let mut reader = oxoffice_core::xml_reader::Reader::from_reader(xml);
    loop {
        let event = reader
            .read_event()
            .map_err(|error| invalid_shape_mutation("read XML", error.to_string()))?;
        let empty = matches!(&event, XmlEvent::Empty(_));
        match event {
            XmlEvent::Start(mut root) | XmlEvent::Empty(mut root) => {
                for (key, uri) in [("xmlns:a", A_NS), ("xmlns:p", P_NS), ("xmlns:r", R_NS)] {
                    let present = root.attributes().any(|attribute| {
                        attribute.is_ok_and(|attribute| attribute.key.as_ref() == key.as_bytes())
                    });
                    if !present {
                        root.push_attribute((key, uri));
                    }
                }
                let mut writer = XmlWriter::new(Vec::new());
                writer
                    .write_event(if empty {
                        XmlEvent::Empty(root)
                    } else {
                        XmlEvent::Start(root)
                    })
                    .map_err(|error| invalid_shape_mutation("read XML", error.to_string()))?;
                let mut result = writer.into_inner();
                result.extend_from_slice(&xml[reader.buffer_position() as usize..]);
                return Ok(result);
            }
            XmlEvent::Eof => return Err(invalid_shape_mutation("read XML", "owner XML is absent")),
            _ => {}
        }
    }
}

fn ppt_xml_table(child: &ShapeTreeChild) -> Result<&CT_Table> {
    if let ShapeTreeChild::GraphicFrame(frame) = child
        && let GraphicDataPayload::Table(table) = frame.graphic_data.payload()
    {
        return Ok(table);
    }
    Err(invalid_shape_mutation(
        "element XML",
        "shape owner is not a table",
    ))
}

fn ppt_xml_table_mut(child: &mut ShapeTreeChild) -> Result<&mut CT_Table> {
    if let ShapeTreeChild::GraphicFrame(frame) = child
        && let Some(table) = frame.graphic_data.table_mut()
    {
        return Ok(table);
    }
    Err(invalid_shape_mutation(
        "element XML",
        "shape owner is not a table",
    ))
}

fn ppt_xml_body(child: &ShapeTreeChild, cell: Option<(usize, usize)>) -> Result<&CT_TextBody> {
    let body = if let Some((row, column)) = cell {
        ppt_xml_table(child)?
            .rows
            .get(row)
            .and_then(|row| row.cells.get(column))
            .and_then(|cell| cell.text_body.as_ref())
    } else if let ShapeTreeChild::Shape(shape) = child {
        shape.text_body.as_ref()
    } else {
        None
    };
    body.ok_or_else(|| invalid_shape_mutation("element XML", "physical text body owner is absent"))
}

fn ppt_xml_body_mut(
    child: &mut ShapeTreeChild,
    cell: Option<(usize, usize)>,
) -> Result<&mut CT_TextBody> {
    let body = if let Some((row, column)) = cell {
        ppt_xml_table_mut(child)?
            .rows
            .get_mut(row)
            .and_then(|row| row.cells.get_mut(column))
            .and_then(|cell| cell.text_body.as_mut())
    } else if let ShapeTreeChild::Shape(shape) = child {
        shape.text_body.as_mut()
    } else {
        None
    };
    body.ok_or_else(|| invalid_shape_mutation("element XML", "physical text body owner is absent"))
}

// Order is checked only for modeled children in their real namespace. Foreign
// payload is preserved, rather than treated as a same-local-name modeled child.
pub(super) fn ppt_xml_child_order(
    namespace: &str,
    parent: &[u8],
    child: &[u8],
) -> Option<(usize, bool)> {
    let (names, repeated): (&[&[u8]], &[&[u8]]) = match (namespace, parent) {
        (A_NS, b"p") => (
            &[b"pPr", b"r", b"br", b"fld", b"endParaRPr"],
            &[b"r", b"br", b"fld"],
        ),
        (A_NS, b"r") => (&[b"rPr", b"t"], &[]),
        (A_NS, b"rPr" | b"endParaRPr") => (
            &[
                b"ln",
                b"noFill",
                b"solidFill",
                b"gradFill",
                b"blipFill",
                b"pattFill",
                b"grpFill",
                b"effectLst",
                b"effectDag",
                b"highlight",
                b"uLnTx",
                b"uLn",
                b"uFillTx",
                b"uFill",
                b"latin",
                b"ea",
                b"cs",
                b"sym",
                b"hlinkClick",
                b"hlinkMouseOver",
                b"rtl",
                b"extLst",
            ],
            &[],
        ),
        (P_NS, b"spPr") => (
            &[
                b"xfrm",
                b"custGeom",
                b"prstGeom",
                b"noFill",
                b"solidFill",
                b"gradFill",
                b"blipFill",
                b"pattFill",
                b"grpFill",
                b"ln",
                b"effectLst",
                b"effectDag",
                b"scene3d",
                b"sp3d",
                b"extLst",
            ],
            &[],
        ),
        (P_NS, b"grpSpPr") => (
            &[b"xfrm", b"effectLst", b"effectDag", b"scene3d", b"extLst"],
            &[],
        ),
        (A_NS, b"ln") => (
            &[
                b"noFill",
                b"solidFill",
                b"gradFill",
                b"pattFill",
                b"prstDash",
                b"custDash",
                b"round",
                b"bevel",
                b"miter",
                b"headEnd",
                b"tailEnd",
                b"extLst",
            ],
            &[],
        ),
        (A_NS, b"tbl") => (&[b"tblPr", b"tblGrid", b"tr"], &[b"tr"]),
        (A_NS, b"tr") => (&[b"tc"], &[b"tc"]),
        (A_NS, b"tblGrid") => (&[b"gridCol"], &[b"gridCol"]),
        (A_NS, b"graphic") => (&[b"graphicData"], &[]),
        (A_NS, b"tc") => (&[b"txBody", b"tcPr"], &[]),
        (A_NS | P_NS, b"txBody") => (&[b"bodyPr", b"lstStyle", b"p"], &[b"p"]),
        (P_NS, b"cSld") => (
            &[b"bg", b"spTree", b"custDataLst", b"controls", b"extLst"],
            &[],
        ),
        (P_NS, b"spTree" | b"grpSp") => (
            &[
                b"nvGrpSpPr",
                b"grpSpPr",
                b"sp",
                b"pic",
                b"graphicFrame",
                b"cxnSp",
                b"grpSp",
                b"extLst",
            ],
            &[b"sp", b"pic", b"graphicFrame", b"cxnSp", b"grpSp"],
        ),
        (P_NS, b"sp") => (&[b"nvSpPr", b"spPr", b"style", b"txBody", b"extLst"], &[]),
        (P_NS, b"pic") => (
            &[b"nvPicPr", b"blipFill", b"spPr", b"style", b"extLst"],
            &[],
        ),
        (P_NS, b"cxnSp") => (&[b"nvCxnSpPr", b"spPr", b"style", b"extLst"], &[]),
        (P_NS, b"graphicFrame") => (&[b"nvGraphicFramePr", b"xfrm", b"graphic", b"extLst"], &[]),
        (P_NS, b"sld") => (
            &[b"cSld", b"clrMapOvr", b"transition", b"timing", b"extLst"],
            &[],
        ),
        (P_NS, b"sldLayout") => (
            &[
                b"cSld",
                b"clrMapOvr",
                b"transition",
                b"timing",
                b"hf",
                b"extLst",
            ],
            &[],
        ),
        (P_NS, b"sldMaster") => (
            &[
                b"cSld",
                b"clrMap",
                b"sldLayoutIdLst",
                b"transition",
                b"timing",
                b"hf",
                b"txStyles",
                b"extLst",
            ],
            &[],
        ),
        _ => return None,
    };
    names.iter().position(|name| *name == child).map(|index| {
        let rank =
            if namespace == A_NS && parent == b"p" {
                match child {
                    b"r" | b"br" | b"fld" => 1,
                    b"endParaRPr" => 2,
                    _ => 0,
                }
            } else if namespace == P_NS && parent == b"spPr" {
                match child {
                    b"xfrm" => 0,
                    b"custGeom" | b"prstGeom" => 1,
                    b"noFill" | b"solidFill" | b"gradFill" | b"blipFill" | b"pattFill"
                    | b"grpFill" => 2,
                    b"ln" => 3,
                    b"effectLst" | b"effectDag" => 4,
                    b"scene3d" => 5,
                    b"sp3d" => 6,
                    _ => 7,
                }
            } else if namespace == P_NS && parent == b"grpSpPr" {
                match child {
                    b"xfrm" => 0,
                    b"effectLst" | b"effectDag" => 1,
                    b"scene3d" => 2,
                    _ => 3,
                }
            } else if namespace == A_NS && parent == b"ln" {
                match child {
                    b"noFill" | b"solidFill" | b"gradFill" | b"pattFill" => 0,
                    b"prstDash" | b"custDash" => 1,
                    b"round" | b"bevel" | b"miter" => 2,
                    b"headEnd" => 3,
                    b"tailEnd" => 4,
                    _ => 5,
                }
            } else if namespace == A_NS && matches!(parent, b"rPr" | b"endParaRPr") {
                match child {
                    b"ln" => 0,
                    b"noFill" | b"solidFill" | b"gradFill" | b"blipFill" | b"pattFill"
                    | b"grpFill" => 1,
                    b"effectLst" | b"effectDag" => 2,
                    b"highlight" => 3,
                    b"uLnTx" | b"uLn" => 4,
                    b"uFillTx" | b"uFill" => 5,
                    b"latin" => 6,
                    b"ea" => 7,
                    b"cs" => 8,
                    b"sym" => 9,
                    b"hlinkClick" => 10,
                    b"hlinkMouseOver" => 11,
                    b"rtl" => 12,
                    _ => 13,
                }
            } else if namespace == P_NS && matches!(parent, b"spTree" | b"grpSp") {
                match child {
                    b"nvGrpSpPr" => 0,
                    b"grpSpPr" => 1,
                    b"extLst" => 3,
                    _ => 2,
                }
            } else {
                index
            };
        (rank, repeated.contains(&child))
    })
}

pub(super) const PPT_MC_NS: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
impl PptXmlNames {
    pub(super) fn id(&mut self, value: &str) -> usize {
        if let Some(id) = self.ids.get(value) {
            return *id;
        }
        let id = self.values.len();
        self.values.push(value.to_owned());
        self.ids.insert(value.to_owned(), id);
        id
    }
    pub(super) fn path(&self, mut route: usize) -> String {
        let mut segments = Vec::new();
        while route != 0 {
            let (parent, ordinal, uri, local) = self.route_edges[route - 1];
            segments.push(format!(
                "{{{}}}{}[{}]",
                self.values[uri],
                self.values[local],
                ordinal + 1
            ));
            route = parent;
        }
        segments.reverse();
        format!("/{}", segments.join("/"))
    }
    pub(super) fn route(
        &mut self,
        parent: usize,
        ordinal: usize,
        uri: usize,
        local: usize,
    ) -> usize {
        let key = (parent, ordinal, uri, local);
        if let Some(route) = self.routes.get(&key) {
            return *route;
        }
        let next = self.route_edges.len() + 1;
        self.route_edges.push(key);
        self.routes.insert(key, next);
        next
    }
}
#[derive(Default)]
pub(super) struct PptXmlScopeDelta {
    bindings: Vec<(String, Option<usize>)>,
    ignorable: Vec<usize>,
}
impl PptXmlScope {
    pub(super) fn name(
        &self,
        raw: &[u8],
        attribute: bool,
        names: &mut PptXmlNames,
    ) -> Result<(usize, usize)> {
        let text = std::str::from_utf8(raw)
            .map_err(|e| invalid_shape_mutation("replace XML", e.to_string()))?;
        let (prefix, local) = text.split_once(':').unwrap_or(("", text));
        let uri = if prefix == "xml" {
            names.id("http://www.w3.org/XML/1998/namespace")
        } else if prefix.is_empty() && attribute {
            names.id("")
        } else if let Some(uri) = self.bindings.get(prefix) {
            *uri
        } else if prefix.is_empty() {
            names.id("")
        } else {
            return Err(invalid_shape_mutation("replace XML", "unbound XML prefix"));
        };
        Ok((uri, names.id(local)))
    }
    pub(super) fn enter(
        &mut self,
        element: &XmlBytesStart<'_>,
        names: &mut PptXmlNames,
    ) -> Result<PptXmlScopeDelta> {
        let mut delta = PptXmlScopeDelta::default();
        for attr in element.attributes() {
            let attr = attr.map_err(|e| invalid_shape_mutation("replace XML", e.to_string()))?;
            let raw = attr.key.as_ref();
            let prefix = if raw == b"xmlns" {
                ""
            } else if let Some(prefix) = raw.strip_prefix(b"xmlns:") {
                std::str::from_utf8(prefix)
                    .map_err(|e| invalid_shape_mutation("replace XML", e.to_string()))?
            } else {
                continue;
            };
            let value = attr
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                .map_err(|e| invalid_shape_mutation("replace XML", e.to_string()))?;
            delta.bindings.push((
                prefix.to_owned(),
                self.bindings.insert(prefix.to_owned(), names.id(&value)),
            ));
        }
        let mc = names.id(PPT_MC_NS);
        let ignorable = names.id("Ignorable");
        for attr in element.attributes() {
            let attr = attr.map_err(|e| invalid_shape_mutation("replace XML", e.to_string()))?;
            if attr.key.as_ref() == b"xmlns" || attr.key.as_ref().starts_with(b"xmlns:") {
                continue;
            }
            if self.name(attr.key.as_ref(), true, names)? == (mc, ignorable) {
                let value = attr
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                    .map_err(|e| invalid_shape_mutation("replace XML", e.to_string()))?;
                for prefix in value.split_ascii_whitespace() {
                    let uri = *self.bindings.get(prefix).ok_or_else(|| {
                        invalid_shape_mutation("replace XML", "unbound mc:Ignorable prefix")
                    })?;
                    if self.ignorable.insert(uri) {
                        delta.ignorable.push(uri);
                    }
                }
            }
        }
        Ok(delta)
    }
    pub(super) fn leave(&mut self, delta: PptXmlScopeDelta) {
        for uri in delta.ignorable {
            self.ignorable.remove(&uri);
        }
        for (prefix, previous) in delta.bindings.into_iter().rev() {
            if let Some(previous) = previous {
                self.bindings.insert(prefix, previous);
            } else {
                self.bindings.remove(&prefix);
            }
        }
    }
}
#[derive(PartialEq, Eq, Hash)]
pub(super) struct PptXmlPolicy {
    route: usize,
    owner: usize,
    name: (usize, usize),
    value: Vec<u8>,
    meaning: Vec<(usize, usize)>,
}
struct PptXmlFrame {
    owner: usize,
    owner_index: Option<usize>,
    uri: usize,
    local: usize,
    route: usize,
    children: usize,
    modeled: bool,
    seen: HashSet<usize>,
    rank: Option<usize>,
    start: usize,
    meaning: usize,
    violation: Option<&'static str>,
    captured: bool,
    scope: PptXmlScopeDelta,
}
fn ppt_xml_frontier(uri: &str, local: &str) -> bool {
    matches!(
        (uri, local),
        (P_NS, "sp" | "sld") | (A_NS, "p" | "prstGeom")
    )
}

/// The qualified nonvisual-property child that carries a physical shape
/// kind's `cNvPr`. The frontier scanner and the stored-package scanner apply
/// the same five-kind match.
pub(super) fn ppt_shape_nonvisual_parent(shape_local: &str) -> &'static str {
    match shape_local {
        "sp" => "nvSpPr",
        "pic" => "nvPicPr",
        "cxnSp" => "nvCxnSpPr",
        "graphicFrame" => "nvGraphicFramePr",
        "grpSp" => "nvGrpSpPr",
        _ => "",
    }
}
fn ppt_xml_frontier_order(
    uri: &str,
    local: &str,
    child_uri: &str,
    child: &str,
) -> Option<(usize, bool)> {
    if (uri, local) == (A_NS, "prstGeom") {
        return (child_uri == A_NS && child == "avLst").then_some((0, false));
    }
    let expected = if uri == P_NS && matches!(local, "txBody" | "spPr" | "grpSpPr")
        || uri == P_NS && local == "graphicFrame" && child == "graphic"
    {
        A_NS
    } else {
        uri
    };
    (child_uri == expected)
        .then(|| ppt_xml_child_order(uri, local.as_bytes(), child.as_bytes()))
        .flatten()
}
pub(super) fn ppt_xml_frontiers(source: &[u8], names: &mut PptXmlNames) -> Result<PptXmlFrontiers> {
    let mut reader = oxoffice_core::xml_reader::Reader::from_reader(source);
    let mut scope = PptXmlScope::default();
    let mut stack: Vec<PptXmlFrame> = Vec::new();
    let mut result = PptXmlFrontiers::default();
    // Supported physical shape records only. Ancestor identity is interned once
    // after parsing, because imported nonvisual properties may appear late.
    let mut owners: Vec<(usize, usize, usize, Option<usize>, bool)> = Vec::new();
    loop {
        let start = reader.buffer_position() as usize;
        let event = reader
            .read_event()
            .map_err(|e| invalid_shape_mutation("replace XML", e.to_string()))?;
        let empty = matches!(event, XmlEvent::Empty(_));
        match event {
            XmlEvent::Start(element) | XmlEvent::Empty(element) => {
                let delta = scope.enter(&element, names)?;
                let (uri, local) = scope.name(element.name().as_ref(), false, names)?;
                let (parent, ordinal) = stack.last_mut().map_or((0, 0), |p| {
                    let n = p.children;
                    p.children += 1;
                    (p.route, n)
                });
                let route = names.route(parent, ordinal, uri, local);
                let mut modeled = stack.is_empty();
                let captured = stack
                    .last()
                    .is_some_and(|p| p.captured || p.violation.is_some());
                let mut violation = None;
                if let Some(p) = stack.last_mut() {
                    let order = ppt_xml_frontier_order(
                        &names.values[p.uri],
                        &names.values[p.local],
                        &names.values[uri],
                        &names.values[local],
                    );
                    // Complex MC is a retained delegated provider, not an
                    // ordinary typed shape branch. Its metadata is still
                    // captured below for exact source/namespace custody.
                    let table = names.values[p.uri] == A_NS
                        && names.values[p.local] == "graphicData"
                        && names.values[uri] == A_NS
                        && names.values[local] == "tbl";
                    let alternate = (names.values[p.uri] == P_NS
                        && matches!(names.values[p.local].as_str(), "spTree" | "grpSp")
                        || names.values[p.uri] == PPT_MC_NS && names.values[p.local] == "Fallback")
                        && names.values[uri] == PPT_MC_NS
                        && names.values[local] == "AlternateContent";
                    let fallback = names.values[p.uri] == PPT_MC_NS
                        && names.values[p.local] == "AlternateContent"
                        && names.values[uri] == PPT_MC_NS
                        && names.values[local] == "Fallback";
                    let fallback_shape = names.values[p.uri] == PPT_MC_NS
                        && names.values[p.local] == "Fallback"
                        && names.values[uri] == P_NS
                        && matches!(
                            names.values[local].as_str(),
                            "sp" | "pic" | "graphicFrame" | "cxnSp" | "grpSp"
                        );
                    modeled = p.modeled
                        && (order.is_some() || table || alternate || fallback || fallback_shape);
                    if p.modeled && ppt_xml_frontier(&names.values[p.uri], &names.values[p.local]) {
                        if let Some((rank, repeated)) = order {
                            if p.rank
                                .is_some_and(|last| rank < last || rank == last && !repeated)
                                || (!p.seen.insert(local) && !repeated)
                            {
                                violation =
                                    Some("finite XML child order or multiplicity is invalid");
                            }
                            p.rank = Some(rank);
                        } else if !(names.values[uri] == PPT_MC_NS
                            && names.values[local] == "AlternateContent")
                            && (matches!(names.values[uri].as_str(), P_NS | A_NS)
                                || !scope.ignorable.contains(&uri))
                        {
                            violation = Some("unknown or misplaced direct XML child");
                        }
                    }
                }
                let inherited_owner = stack.last().map_or(0, |frame| frame.owner);
                let physical_shape = modeled
                    && names.values[uri] == P_NS
                    && matches!(
                        names.values[local].as_str(),
                        "sp" | "pic" | "cxnSp" | "graphicFrame" | "grpSp"
                    );
                let owner_index = physical_shape.then(|| {
                    let index = owners.len();
                    owners.push((route, inherited_owner, local, None, false));
                    index
                });
                let owner = if physical_shape {
                    route
                } else {
                    inherited_owner
                };
                // A cNvPr identifies only its actual supported enclosing shape,
                // through that shape kind's qualified nonvisual-property child.
                if names.values[uri] == P_NS && names.values[local] == "cNvPr" && stack.len() >= 2 {
                    let shape = &stack[stack.len() - 2];
                    let parent = &stack[stack.len() - 1];
                    let expected = ppt_shape_nonvisual_parent(&names.values[shape.local]);
                    if let Some(index) = shape.owner_index
                        && names.values[parent.uri] == P_NS
                        && names.values[parent.local] == expected
                    {
                        let record = &mut owners[index];
                        let mut id = None;
                        for attr in element.attributes() {
                            let attr = attr.map_err(|e| {
                                invalid_shape_mutation("replace XML", e.to_string())
                            })?;
                            if attr.key.as_ref() == b"xmlns"
                                || attr.key.as_ref().starts_with(b"xmlns:")
                            {
                                continue;
                            }
                            if scope.name(attr.key.as_ref(), true, names)?
                                == (names.id(""), names.id("id"))
                            {
                                let value = attr
                                    .decoded_and_normalized_value(
                                        XmlVersion::Implicit1_0,
                                        element.decoder(),
                                    )
                                    .map_err(|e| {
                                        invalid_shape_mutation("replace XML", e.to_string())
                                    })?;
                                id = value
                                    .parse::<u32>()
                                    .ok()
                                    .filter(|id| *id != 0)
                                    .map(|id| names.id(&id.to_string()));
                            }
                        }
                        record.3 = if record.4 { None } else { id };
                        record.4 = true;
                    }
                }
                let meaning_start = result.meaning.len();
                result.meaning.push((uri, local));
                let mc = names.id(PPT_MC_NS);
                let ignorable = names.id("Ignorable");
                let requires = names.id("Requires");
                for attr in element.attributes() {
                    let attr =
                        attr.map_err(|e| invalid_shape_mutation("replace XML", e.to_string()))?;
                    if attr.key.as_ref() == b"xmlns" || attr.key.as_ref().starts_with(b"xmlns:") {
                        continue;
                    }
                    let name = scope.name(attr.key.as_ref(), true, names)?;
                    result.meaning.push(name);
                    let value = attr
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())
                        .map_err(|e| invalid_shape_mutation("replace XML", e.to_string()))?;
                    let choice_requires = uri == mc
                        && names.values[local] == "Choice"
                        && names.values[name.0].is_empty()
                        && name.1 == requires;
                    let mut meaning = Vec::new();
                    if name.0 == mc || choice_requires {
                        for token in value.split_ascii_whitespace() {
                            let expanded = if name.1 == ignorable
                                || names.values[name.1] == "MustUnderstand"
                                || choice_requires
                            {
                                let target = *scope.bindings.get(token).ok_or_else(|| {
                                    invalid_shape_mutation(
                                        "replace XML",
                                        "unbound compatibility prefix",
                                    )
                                })?;
                                (target, names.id(""))
                            } else {
                                scope.name(token.as_bytes(), false, names)?
                            };
                            meaning.push(expanded);
                        }
                        result.meaning.extend_from_slice(&meaning);
                        if name.1 != ignorable {
                            result.policies.push(PptXmlPolicy {
                                route,
                                owner,
                                name,
                                value: attr.value.to_vec(),
                                meaning,
                            });
                        }
                    }
                }
                // A new complex compatibility route is unsupported. Preserve only
                // existing element metadata, allowing typed descendants to change.
                if uri == mc {
                    result.policies.push(PptXmlPolicy {
                        route,
                        owner,
                        name: (uri, local),
                        value: Vec::new(),
                        meaning: Vec::new(),
                    });
                }
                stack.push(PptXmlFrame {
                    owner,
                    owner_index,
                    uri,
                    local,
                    route,
                    children: 0,
                    modeled,
                    seen: HashSet::new(),
                    rank: None,
                    start,
                    meaning: meaning_start,
                    violation,
                    captured,
                    scope: delta,
                });
                if empty {
                    ppt_xml_finish_frontier(
                        &mut stack,
                        &mut scope,
                        &mut result,
                        names,
                        reader.buffer_position() as usize,
                    );
                }
            }
            XmlEvent::End(_) => ppt_xml_finish_frontier(
                &mut stack,
                &mut scope,
                &mut result,
                names,
                reader.buffer_position() as usize,
            ),
            XmlEvent::Eof => {
                let mut identities = HashMap::from([(0, 0)]);
                for (route, parent, local, id, _) in owners {
                    let parent = identities[&parent];
                    let identity = if let Some(id) = id.filter(|_| parent != usize::MAX) {
                        let next = names.physical_owners.len() + 1;
                        *names
                            .physical_owners
                            .entry((parent, local, id))
                            .or_insert(next)
                    } else {
                        usize::MAX
                    };
                    identities.insert(route, identity);
                }
                for island in &mut result.islands {
                    island.owner = identities[&island.owner];
                }
                for policy in &mut result.policies {
                    policy.owner = identities[&policy.owner];
                }
                return Ok(result);
            }
            _ => {}
        }
    }
}
fn ppt_xml_finish_frontier(
    stack: &mut Vec<PptXmlFrame>,
    scope: &mut PptXmlScope,
    result: &mut PptXmlFrontiers,
    names: &PptXmlNames,
    end: usize,
) {
    let Some(mut frame) = stack.pop() else {
        return;
    };
    if frame.modeled && names.values[frame.uri] == P_NS {
        let required: &[&str] = match names.values[frame.local].as_str() {
            "sp" => &["nvSpPr", "spPr"],
            "sld" => &["cSld"],
            _ => &[],
        };
        if required
            .iter()
            .any(|required| !frame.seen.iter().any(|id| names.values[*id] == *required))
        {
            frame.violation = Some("required finite XML child is absent");
        }
    }
    if !frame.captured
        && let Some(message) = frame.violation
    {
        result.islands.push(PptXmlIsland {
            route: frame.route,
            owner: frame.owner,
            source: frame.start..end,
            meaning: frame.meaning..result.meaning.len(),
            message,
        });
    }
    scope.leave(frame.scope);
}
fn check_ppt_frontier_changes(before: &[u8], after: &[u8]) -> Result<()> {
    let mut names = PptXmlNames::default();
    let old = ppt_xml_frontiers(before, &mut names)?;
    let new = ppt_xml_frontiers(after, &mut names)?;
    let mut old_islands: HashMap<usize, Vec<&PptXmlIsland>> = HashMap::new();
    for island in &old.islands {
        old_islands.entry(island.route).or_default().push(island);
    }
    for island in &new.islands {
        let previous = old_islands
            .get(&island.route)
            .filter(|values| values.len() == 1)
            .map(|values| values[0]);
        if !previous.is_some_and(|previous| {
            island.owner != usize::MAX
                && previous.owner == island.owner
                && before[previous.source.clone()] == after[island.source.clone()]
                && old.meaning[previous.meaning.clone()] == new.meaning[island.meaning.clone()]
        }) {
            return Err(invalid_shape_mutation("replace XML", island.message));
        }
    }
    let mut old_policies: HashMap<&PptXmlPolicy, usize> = HashMap::new();
    for policy in &old.policies {
        *old_policies.entry(policy).or_default() += 1;
    }
    for policy in &new.policies {
        if policy.owner == usize::MAX || old_policies.get(policy) != Some(&1) {
            return Err(invalid_shape_mutation(
                "replace XML",
                "new or reinterpreted complex markup compatibility metadata is unsupported",
            ));
        }
    }
    Ok(())
}

// Locate only the selected supported physical owner. This is not an opaque
// inventory: one live ancestry stack and direct-name ordinals supply context.
struct PptXmlSourceFrame {
    uri: usize,
    local: usize,
    ordinal: usize,
    start: usize,
    children: HashMap<(usize, usize), usize>,
    structural: bool,
    shape: bool,
    scope: PptXmlScopeDelta,
}
fn ppt_xml_owner_range(
    source: &[u8],
    id: u32,
    shape_root: &[u8],
    steps: &[(&str, &str, usize)],
) -> Result<std::ops::Range<usize>> {
    let mut reader = oxoffice_core::xml_reader::Reader::from_reader(source);
    let mut names = PptXmlNames::default();
    let mut scope = PptXmlScope::default();
    let mut stack: Vec<PptXmlSourceFrame> = Vec::new();
    let mut anchor = None;
    let mut found = 0;
    let mut selected = None;
    loop {
        let start = reader.buffer_position() as usize;
        let event = reader
            .read_event()
            .map_err(|e| invalid_shape_mutation("replace XML", e.to_string()))?;
        let empty = matches!(event, XmlEvent::Empty(_));
        match event {
            XmlEvent::Start(element) | XmlEvent::Empty(element) => {
                let delta = scope.enter(&element, &mut names)?;
                let (uri, local) = scope.name(element.name().as_ref(), false, &mut names)?;
                let ordinal = stack.last_mut().map_or(0, |parent| {
                    let count = parent.children.entry((uri, local)).or_default();
                    let ordinal = *count;
                    *count += 1;
                    ordinal
                });
                let p = names.values[uri] == P_NS;
                let tag = names.values[local].as_str();
                let structural = p
                    && (stack.is_empty() && matches!(tag, "sld" | "sldLayout" | "sldMaster")
                        || stack.last().is_some_and(|parent| {
                            parent.structural
                                && (matches!(
                                    names.values[parent.local].as_str(),
                                    "sld" | "sldLayout" | "sldMaster"
                                ) && tag == "cSld"
                                    || names.values[parent.local] == "cSld" && tag == "spTree"
                                    || matches!(
                                        names.values[parent.local].as_str(),
                                        "spTree" | "grpSp"
                                    ) && tag == "grpSp")
                        }));
                let shape =
                    p && stack.last().is_some_and(|parent| {
                        parent.structural
                            && matches!(names.values[parent.local].as_str(), "spTree" | "grpSp")
                    }) && matches!(tag, "sp" | "pic" | "graphicFrame" | "cxnSp" | "grpSp");
                if p && tag == "cNvPr" && stack.len() >= 2 {
                    let parent = &stack[stack.len() - 1];
                    let root = &stack[stack.len() - 2];
                    if root.shape
                        && names.values[root.local].as_bytes() == shape_root
                        && names.values[parent.uri] == P_NS
                        && matches!(
                            names.values[parent.local].as_str(),
                            "nvSpPr" | "nvPicPr" | "nvGraphicFramePr" | "nvCxnSpPr" | "nvGrpSpPr"
                        )
                    {
                        for attr in element.attributes() {
                            let attr = attr.map_err(|e| {
                                invalid_shape_mutation("replace XML", e.to_string())
                            })?;
                            if attr.key.as_ref() == b"id"
                                && attr
                                    .decoded_and_normalized_value(
                                        XmlVersion::Implicit1_0,
                                        element.decoder(),
                                    )
                                    .map_err(|e| {
                                        invalid_shape_mutation("replace XML", e.to_string())
                                    })?
                                    .parse::<u32>()
                                    .ok()
                                    == Some(id)
                            {
                                found += 1;
                                anchor = Some((stack.len() - 2, root.start));
                            }
                        }
                    }
                }
                stack.push(PptXmlSourceFrame {
                    uri,
                    local,
                    ordinal,
                    start,
                    children: HashMap::new(),
                    structural,
                    shape,
                    scope: delta,
                });
                if empty {
                    ppt_xml_finish_owner_range(
                        &mut stack,
                        &mut scope,
                        &names,
                        anchor,
                        steps,
                        reader.buffer_position() as usize,
                        &mut selected,
                    )?;
                }
            }
            XmlEvent::End(_) => ppt_xml_finish_owner_range(
                &mut stack,
                &mut scope,
                &names,
                anchor,
                steps,
                reader.buffer_position() as usize,
                &mut selected,
            )?,
            XmlEvent::Eof => break,
            _ => {}
        }
    }
    if found != 1 {
        return Err(invalid_shape_mutation(
            "replace XML",
            "physical shape source identity is absent or ambiguous",
        ));
    }
    selected.ok_or_else(|| {
        invalid_shape_mutation("replace XML", "selected physical source owner is absent")
    })
}
fn ppt_xml_finish_owner_range(
    stack: &mut Vec<PptXmlSourceFrame>,
    scope: &mut PptXmlScope,
    names: &PptXmlNames,
    anchor: Option<(usize, usize)>,
    steps: &[(&str, &str, usize)],
    end: usize,
    selected: &mut Option<std::ops::Range<usize>>,
) -> Result<()> {
    if let Some((anchor, root_start)) = anchor
        && stack.len() == anchor + 1 + steps.len()
        && stack[anchor].start == root_start
        && stack[anchor + 1..]
            .iter()
            .zip(steps)
            .all(|(frame, (uri, local, ordinal))| {
                names.values[frame.uri] == *uri
                    && names.values[frame.local] == *local
                    && frame.ordinal == *ordinal
            })
        && let Some(frame) = stack.last()
        && selected.replace(frame.start..end).is_some()
    {
        return Err(invalid_shape_mutation(
            "replace XML",
            "selected physical source range is ambiguous",
        ));
    }
    if let Some(frame) = stack.pop() {
        scope.leave(frame.scope);
    }
    Ok(())
}

fn check_ppt_xml_input(xml: &[u8], expected_namespace: &str, expected_local: &[u8]) -> Result<()> {
    oxoffice_core::xml::validate_strict_xml_1_0(xml)
        .map_err(|error| invalid_shape_mutation("replace XML", format!("{error:?}")))?;
    let mut reader = NsReader::from_reader(xml);
    type Frame = (
        String,
        Vec<u8>,
        Option<usize>,
        HashSet<Vec<u8>>,
        bool,
        usize,
    );
    let mut stack: Vec<Frame> = Vec::new();
    let mut roots = 0;
    loop {
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|error| invalid_shape_mutation("replace XML", error.to_string()))?;
        let empty = matches!(&event, XmlEvent::Empty(_));
        match event {
            XmlEvent::Start(element) | XmlEvent::Empty(element) => {
                let namespace = match namespace {
                    XmlResolveResult::Bound(namespace) => {
                        import_namespace_uri(XmlResolveResult::Bound(namespace), element.decoder())
                            .map_err(|error| {
                                invalid_shape_mutation("replace XML", error.to_string())
                            })?
                    }
                    XmlResolveResult::Unbound => String::new(),
                    _ => {
                        return Err(invalid_shape_mutation(
                            "replace XML",
                            "element prefix is not bound",
                        ));
                    }
                };
                let local = element.local_name().as_ref().to_vec();
                let mut modeled = true;
                let mut nesting = 0;
                if stack.is_empty() {
                    roots += 1;
                    if roots != 1 || namespace != expected_namespace || local != expected_local {
                        return Err(invalid_shape_mutation(
                            "replace XML",
                            "expected one complete qualified owner root",
                        ));
                    }
                } else if let Some((
                    parent_namespace,
                    parent,
                    last,
                    seen,
                    parent_modeled,
                    parent_nesting,
                )) = stack.last_mut()
                {
                    nesting = *parent_nesting;
                    let child_namespace = if parent_namespace == P_NS
                        && (matches!(parent.as_slice(), b"txBody" | b"spPr" | b"grpSpPr")
                            || parent == b"graphicFrame" && local == b"graphic")
                    {
                        A_NS
                    } else {
                        parent_namespace.as_str()
                    };
                    let order = (namespace == child_namespace)
                        .then(|| ppt_xml_child_order(parent_namespace, parent, &local))
                        .flatten();
                    modeled = *parent_modeled
                        && (order.is_some()
                            || parent_namespace == A_NS
                                && parent == b"graphicData"
                                && namespace == A_NS
                                && local == b"tbl"
                            || parent_namespace
                                == "http://schemas.openxmlformats.org/markup-compatibility/2006"
                                && matches!(
                                    parent.as_slice(),
                                    b"AlternateContent" | b"Choice" | b"Fallback"
                                )
                                && (namespace == P_NS
                                    || namespace
                                        == "http://schemas.openxmlformats.org/markup-compatibility/2006")
                            || parent_namespace == P_NS
                                && matches!(parent.as_slice(), b"spTree" | b"grpSp")
                                && namespace
                                    == "http://schemas.openxmlformats.org/markup-compatibility/2006"
                                && local == b"AlternateContent");
                    if *parent_modeled
                        && !ppt_xml_frontier(parent_namespace, &String::from_utf8_lossy(parent))
                        && let Some((rank, repeated)) = order
                    {
                        if last.is_some_and(|last| rank < last || rank == last && !repeated)
                            || (!seen.insert(local.clone()) && !repeated)
                        {
                            return Err(invalid_shape_mutation(
                                "replace XML",
                                "modeled XML child order or multiplicity is invalid",
                            ));
                        }
                        *last = Some(rank);
                    }
                }
                if modeled
                    && namespace == A_NS
                    && matches!(local.as_slice(), b"rPr" | b"endParaRPr")
                {
                    for attribute in element.attributes() {
                        let attribute = attribute.map_err(|error| {
                            invalid_shape_mutation("replace XML", error.to_string())
                        })?;
                        if attribute.key.as_ref() == b"cap" {
                            let value = attribute
                                .decoded_and_normalized_value(
                                    XmlVersion::Implicit1_0,
                                    reader.decoder(),
                                )
                                .map_err(|error| {
                                    invalid_shape_mutation("replace XML", error.to_string())
                                })?;
                            if !matches!(value.as_ref(), "all" | "small" | "none") {
                                return Err(invalid_shape_mutation(
                                    "replace XML",
                                    "known text cap attribute must be all, small or none",
                                ));
                            }
                        }
                    }
                }
                if modeled
                    && (namespace == P_NS && local == b"grpSp"
                        || namespace
                            == "http://schemas.openxmlformats.org/markup-compatibility/2006"
                            && local == b"AlternateContent")
                {
                    nesting += 1;
                    if nesting > oxoffice_pptx_oxml::MAX_PRESENTATION_NESTING {
                        return Err(invalid_shape_mutation(
                            "replace XML",
                            "group and alternate content nesting exceeds limit 32",
                        ));
                    }
                }
                if modeled
                    && empty
                    && local == b"txBody"
                    && matches!(namespace.as_str(), A_NS | P_NS)
                {
                    return Err(invalid_shape_mutation(
                        "replace XML",
                        "text body requires bodyPr and a physical paragraph",
                    ));
                }
                if !empty {
                    stack.push((namespace, local, None, HashSet::new(), modeled, nesting));
                }
            }
            XmlEvent::End(_) => {
                let (namespace, local, _, seen, modeled, _) = stack
                    .pop()
                    .ok_or_else(|| invalid_shape_mutation("replace XML", "unmatched owner end"))?;
                if modeled
                    && local == b"txBody"
                    && matches!(namespace.as_str(), A_NS | P_NS)
                    && (!seen.contains(b"bodyPr".as_slice()) || !seen.contains(b"p".as_slice()))
                {
                    return Err(invalid_shape_mutation(
                        "replace XML",
                        "text body requires bodyPr and a physical paragraph",
                    ));
                }
            }
            XmlEvent::DocType(_) => {
                return Err(invalid_shape_mutation("replace XML", "DTD is forbidden"));
            }
            XmlEvent::Text(text)
                if !text.iter().all(u8::is_ascii_whitespace)
                    && stack
                        .last()
                        .is_some_and(|(namespace, local, _, _, modeled, _)| {
                            *modeled && !(namespace == A_NS && local == b"t")
                        }) =>
            {
                return Err(invalid_shape_mutation(
                    "replace XML",
                    "modeled container cannot contain character data",
                ));
            }
            XmlEvent::CData(_) | XmlEvent::GeneralRef(_)
                if stack
                    .last()
                    .is_some_and(|(namespace, local, _, _, modeled, _)| {
                        *modeled && !(namespace == A_NS && local == b"t")
                    }) =>
            {
                return Err(invalid_shape_mutation(
                    "replace XML",
                    "modeled container cannot contain character data",
                ));
            }
            XmlEvent::Text(text)
                if stack.is_empty() && !text.iter().all(u8::is_ascii_whitespace) =>
            {
                return Err(invalid_shape_mutation(
                    "replace XML",
                    "text outside owner root",
                ));
            }
            XmlEvent::Eof => {
                if roots != 1 || !stack.is_empty() {
                    return Err(invalid_shape_mutation(
                        "replace XML",
                        "owner root is incomplete",
                    ));
                }
                return Ok(());
            }
            _ => {}
        }
    }
}

// Only concrete text/table vocabulary can establish an alias-only no-op.
// Unknown nodes, attributes and foreign bindings decline equivalence. In
// particular opaque QName values cannot lose their inherited namespace facts.
fn ppt_xml_known_owner_equivalent(before: &[u8], incoming: &[u8]) -> Result<bool> {
    fn tokens(xml: &[u8]) -> Result<Option<Vec<String>>> {
        let mut reader = NsReader::from_reader(xml);
        let mut out = Vec::new();
        loop {
            let (namespace, event) = reader
                .read_resolved_event()
                .map_err(|error| invalid_shape_mutation("replace XML", error.to_string()))?;
            let empty = matches!(&event, XmlEvent::Empty(_));
            match event {
                XmlEvent::Start(element) | XmlEvent::Empty(element) => {
                    let uri = match namespace {
                        XmlResolveResult::Bound(uri) => {
                            String::from_utf8_lossy(uri.as_ref()).into_owned()
                        }
                        _ => return Ok(None),
                    };
                    let name = element.local_name();
                    let local = name.as_ref();
                    if !matches!(uri.as_str(), A_NS | P_NS)
                        || !matches!(
                            local,
                            b"p" | b"r"
                                | b"t"
                                | b"pPr"
                                | b"rPr"
                                | b"endParaRPr"
                                | b"txBody"
                                | b"bodyPr"
                                | b"lstStyle"
                                | b"tbl"
                                | b"tblPr"
                                | b"tblGrid"
                                | b"gridCol"
                                | b"tr"
                                | b"tc"
                                | b"tcPr"
                                | b"latin"
                                | b"ea"
                                | b"cs"
                                | b"sym"
                                | b"hlinkClick"
                                | b"hlinkMouseOver"
                        )
                    {
                        return Ok(None);
                    }
                    let mut attributes = Vec::new();
                    for attribute in element.attributes() {
                        let attribute = attribute.map_err(|error| {
                            invalid_shape_mutation("replace XML", error.to_string())
                        })?;
                        let value = attribute
                            .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                            .map_err(|error| {
                                invalid_shape_mutation("replace XML", error.to_string())
                            })?;
                        if attribute.key.as_ref() == b"xmlns"
                            || attribute.key.as_ref().starts_with(b"xmlns:")
                        {
                            if !matches!(value.as_ref(), A_NS | P_NS | R_NS) {
                                return Ok(None);
                            }
                            continue;
                        }
                        let (namespace, name) = reader.resolver().resolve_attribute(attribute.key);
                        let attribute_uri = match namespace {
                            XmlResolveResult::Bound(uri) => {
                                String::from_utf8_lossy(uri.as_ref()).into_owned()
                            }
                            XmlResolveResult::Unbound => String::new(),
                            _ => return Ok(None),
                        };
                        let known = attribute_uri.is_empty()
                            && match local {
                                b"rPr" | b"endParaRPr" => matches!(
                                    name.as_ref(),
                                    b"b" | b"i"
                                        | b"sz"
                                        | b"lang"
                                        | b"altLang"
                                        | b"kern"
                                        | b"baseline"
                                        | b"dirty"
                                        | b"smtClean"
                                ),
                                b"pPr" => matches!(
                                    name.as_ref(),
                                    b"algn" | b"lvl" | b"marL" | b"marR" | b"indent" | b"rtl"
                                ),
                                b"gridCol" => name.as_ref() == b"w",
                                b"tr" => name.as_ref() == b"h",
                                b"tc" => matches!(
                                    name.as_ref(),
                                    b"rowSpan" | b"gridSpan" | b"hMerge" | b"vMerge"
                                ),
                                b"latin" | b"ea" | b"cs" | b"sym" => matches!(
                                    name.as_ref(),
                                    b"typeface" | b"panose" | b"pitchFamily" | b"charset"
                                ),
                                b"hlinkClick" | b"hlinkMouseOver" => matches!(
                                    name.as_ref(),
                                    b"action"
                                        | b"tooltip"
                                        | b"tgtFrame"
                                        | b"history"
                                        | b"highlightClick"
                                        | b"endSnd"
                                        | b"invalidUrl"
                                ),
                                _ => false,
                            }
                            || attribute_uri == R_NS
                                && matches!(local, b"hlinkClick" | b"hlinkMouseOver")
                                && name.as_ref() == b"id"
                            || attribute_uri == "http://www.w3.org/XML/1998/namespace"
                                && local == b"t"
                                && name.as_ref() == b"space";
                        if !known {
                            return Ok(None);
                        }
                        attributes.push(format!(
                            "{attribute_uri}\0{}\0{value}",
                            String::from_utf8_lossy(name.as_ref())
                        ));
                    }
                    attributes.sort();
                    out.push(format!("start:{uri}:{}", String::from_utf8_lossy(local)));
                    out.extend(attributes);
                    out.push("attributes-end".to_owned());
                    if empty {
                        out.push(format!("end:{uri}:{}", String::from_utf8_lossy(local)));
                    }
                }
                XmlEvent::End(element) => {
                    let uri = match namespace {
                        XmlResolveResult::Bound(uri) => {
                            String::from_utf8_lossy(uri.as_ref()).into_owned()
                        }
                        _ => return Ok(None),
                    };
                    out.push(format!(
                        "end:{uri}:{}",
                        String::from_utf8_lossy(element.local_name().as_ref())
                    ));
                }
                XmlEvent::Text(text) => {
                    out.push(format!("text:{}", String::from_utf8_lossy(&text)))
                }
                XmlEvent::Eof => return Ok(Some(out)),
                _ => return Ok(None),
            }
        }
    }
    Ok(
        matches!((tokens(before)?, tokens(incoming)?), (Some(before), Some(incoming)) if before == incoming),
    )
}

type PptXmlRelationshipFact = (String, String, String, String, Option<String>);

fn ppt_xml_relationship_facts(xml: &[u8]) -> Result<Vec<PptXmlRelationshipFact>> {
    let mut reader = NsReader::from_reader(xml);
    let mut result = Vec::new();
    loop {
        let (namespace, event) = reader
            .read_resolved_event()
            .map_err(|error| invalid_shape_mutation("replace XML", error.to_string()))?;
        match event {
            XmlEvent::Start(element) | XmlEvent::Empty(element) => {
                let uri = match namespace {
                    XmlResolveResult::Bound(namespace) => {
                        String::from_utf8_lossy(namespace.as_ref()).into_owned()
                    }
                    _ => String::new(),
                };
                let local = String::from_utf8_lossy(element.local_name().as_ref()).into_owned();
                let action = element
                    .attributes()
                    .find_map(|attribute| {
                        attribute
                            .ok()
                            .filter(|attribute| attribute.key.as_ref() == b"action")
                    })
                    .map(|attribute| {
                        attribute
                            .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                            .map(|value| value.into_owned())
                    })
                    .transpose()
                    .map_err(|error| invalid_shape_mutation("replace XML", error.to_string()))?;
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|error| {
                        invalid_shape_mutation("replace XML", error.to_string())
                    })?;
                    let (namespace, name) = reader.resolver().resolve_attribute(attribute.key);
                    if xml_is(&namespace, R_NS) {
                        let value = attribute
                            .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                            .map_err(|error| {
                                invalid_shape_mutation("replace XML", error.to_string())
                            })?
                            .into_owned();
                        result.push((
                            uri.clone(),
                            local.clone(),
                            String::from_utf8_lossy(name.as_ref()).into_owned(),
                            value,
                            action.clone(),
                        ));
                    }
                }
            }
            XmlEvent::Eof => return Ok(result),
            _ => {}
        }
    }
}

impl Presentation {
    /// Reads the namespace-complete XML of a physical shape in its actual part.
    pub fn shape_xml_at(&self, owner: ShapeOwner, path: &[usize]) -> Result<Vec<u8>> {
        self.checked_owner_xml(PptXmlOwner::Shape(owner, path))
    }

    /// Replaces a shape without changing its captured physical kind or identity.
    pub fn replace_shape_xml_at(
        &mut self,
        owner: ShapeOwner,
        path: &[usize],
        xml: &str,
    ) -> Result<()> {
        self.replace_checked_owner_xml(PptXmlOwner::Shape(owner, path), xml)
    }

    pub fn table_xml_at(&self, owner: ShapeOwner, path: &[usize]) -> Result<Vec<u8>> {
        self.checked_owner_xml(PptXmlOwner::Table(owner, path))
    }

    pub fn replace_table_xml_at(
        &mut self,
        owner: ShapeOwner,
        path: &[usize],
        xml: &str,
    ) -> Result<()> {
        self.replace_checked_owner_xml(PptXmlOwner::Table(owner, path), xml)
    }

    pub fn cell_xml_at(
        &self,
        owner: ShapeOwner,
        path: &[usize],
        row: usize,
        column: usize,
    ) -> Result<Vec<u8>> {
        self.checked_owner_xml(PptXmlOwner::Cell(owner, path, row, column))
    }

    pub fn replace_cell_xml_at(
        &mut self,
        owner: ShapeOwner,
        path: &[usize],
        row: usize,
        column: usize,
        xml: &str,
    ) -> Result<()> {
        self.replace_checked_owner_xml(PptXmlOwner::Cell(owner, path, row, column), xml)
    }

    pub fn text_body_xml_at(
        &self,
        owner: ShapeOwner,
        path: &[usize],
        cell: Option<(usize, usize)>,
    ) -> Result<Vec<u8>> {
        self.checked_owner_xml(PptXmlOwner::TextBody(owner, path, cell))
    }

    pub fn replace_text_body_xml_at(
        &mut self,
        owner: ShapeOwner,
        path: &[usize],
        cell: Option<(usize, usize)>,
        xml: &str,
    ) -> Result<()> {
        self.replace_checked_owner_xml(PptXmlOwner::TextBody(owner, path, cell), xml)
    }

    pub fn paragraph_xml_at(
        &self,
        owner: ShapeOwner,
        path: &[usize],
        cell: Option<(usize, usize)>,
        paragraph: usize,
    ) -> Result<Vec<u8>> {
        self.checked_owner_xml(PptXmlOwner::Paragraph(owner, path, cell, paragraph))
    }

    pub fn replace_paragraph_xml_at(
        &mut self,
        owner: ShapeOwner,
        path: &[usize],
        cell: Option<(usize, usize)>,
        paragraph: usize,
        xml: &str,
    ) -> Result<()> {
        self.replace_checked_owner_xml(PptXmlOwner::Paragraph(owner, path, cell, paragraph), xml)
    }

    pub fn run_xml_at(
        &self,
        owner: ShapeOwner,
        path: &[usize],
        cell: Option<(usize, usize)>,
        paragraph: usize,
        run: usize,
    ) -> Result<Vec<u8>> {
        self.checked_owner_xml(PptXmlOwner::Run(owner, path, cell, paragraph, run))
    }

    #[allow(clippy::too_many_arguments)] // Actual part, shape, cell, paragraph and regular-run operands.
    pub fn replace_run_xml_at(
        &mut self,
        owner: ShapeOwner,
        path: &[usize],
        cell: Option<(usize, usize)>,
        paragraph: usize,
        run: usize,
        xml: &str,
    ) -> Result<()> {
        self.replace_checked_owner_xml(PptXmlOwner::Run(owner, path, cell, paragraph, run), xml)
    }

    pub fn slide_xml(&self, index: usize) -> Result<Vec<u8>> {
        self.checked_owner_xml(PptXmlOwner::Part(ShapeOwner::Slide(index)))
    }

    pub fn replace_slide_xml(&mut self, index: usize, xml: &str) -> Result<()> {
        self.replace_checked_owner_xml(PptXmlOwner::Part(ShapeOwner::Slide(index)), xml)
    }

    pub fn layout_xml(&self, index: usize) -> Result<Vec<u8>> {
        self.checked_owner_xml(PptXmlOwner::Part(ShapeOwner::Layout(index)))
    }

    pub fn replace_layout_xml(&mut self, index: usize, xml: &str) -> Result<()> {
        self.replace_checked_owner_xml(PptXmlOwner::Part(ShapeOwner::Layout(index)), xml)
    }

    pub fn master_xml(&self, index: usize) -> Result<Vec<u8>> {
        self.checked_owner_xml(PptXmlOwner::Part(ShapeOwner::Master(index)))
    }

    pub fn replace_master_xml(&mut self, index: usize, xml: &str) -> Result<()> {
        self.replace_checked_owner_xml(PptXmlOwner::Part(ShapeOwner::Master(index)), xml)
    }

    pub(super) fn checked_owner_xml(&self, owner: PptXmlOwner<'_>) -> Result<Vec<u8>> {
        self.owner_data(owner.part())?;
        let result = match owner {
            PptXmlOwner::Part(ShapeOwner::Slide(index)) => self.slides[index].slide.to_xml(),
            PptXmlOwner::Part(ShapeOwner::Layout(index)) => self.layouts[index].layout.to_xml(),
            PptXmlOwner::Part(ShapeOwner::Master(index)) => self.masters[index]
                .master
                .as_ref()
                .map_err(|message| invalid_shape_mutation("read XML", message))?
                .to_xml(),
            PptXmlOwner::Shape(part, path) => {
                return ppt_xml_close_namespaces(&self.part_shape_at(part, path)?.xml()?);
            }
            PptXmlOwner::Table(part, path) => {
                ppt_xml_table(self.part_shape_at(part, path)?.child)?.to_xml()
            }
            PptXmlOwner::Cell(part, path, row, column) => {
                ppt_xml_table(self.part_shape_at(part, path)?.child)?
                    .rows
                    .get(row)
                    .and_then(|row| row.cells.get(column))
                    .ok_or_else(|| invalid_shape_mutation("read XML", "cell owner is absent"))?
                    .to_xml()
            }
            PptXmlOwner::TextBody(part, path, cell) => {
                let body = ppt_xml_body(self.part_shape_at(part, path)?.child, cell)?;
                let mut writer = XmlWriter::new(Vec::new());
                body.write_xml_as(
                    &mut writer,
                    if cell.is_some() {
                        "a:txBody"
                    } else {
                        "p:txBody"
                    },
                )
                .map_err(|error| invalid_shape_mutation("read XML", error.to_string()))?;
                Ok(writer.into_inner())
            }
            PptXmlOwner::Paragraph(part, path, cell, index) => {
                ppt_xml_body(self.part_shape_at(part, path)?.child, cell)?
                    .paragraphs()
                    .get(index)
                    .ok_or_else(|| invalid_shape_mutation("read XML", "paragraph owner is absent"))?
                    .to_xml()
                    .map_err(|error| OxmlError::InvalidValue(error.to_string()))
            }
            PptXmlOwner::Run(part, path, cell, paragraph, index) => {
                ppt_xml_body(self.part_shape_at(part, path)?.child, cell)?
                    .paragraphs()
                    .get(paragraph)
                    .and_then(|paragraph| {
                        paragraph
                            .runs
                            .iter()
                            .filter_map(|run| match run {
                                TextRun::Run(run) => Some(run),
                                _ => None,
                            })
                            .nth(index)
                    })
                    .ok_or_else(|| {
                        invalid_shape_mutation("read XML", "regular run owner is absent")
                    })?
                    .to_xml()
                    .map_err(|error| OxmlError::InvalidValue(error.to_string()))
            }
        };
        ppt_xml_close_namespaces(
            &result.map_err(|error| invalid_shape_mutation("read XML", error.to_string()))?,
        )
    }

    fn xml_physical_owner_range(
        &self,
        owner: PptXmlOwner<'_>,
        source: &[u8],
    ) -> Result<std::ops::Range<usize>> {
        let mut steps = Vec::new();
        let (part, path) = match owner {
            PptXmlOwner::Part(_) => return Ok(0..source.len()),
            PptXmlOwner::Shape(part, path) => (part, path),
            PptXmlOwner::Table(part, path) => {
                steps.extend([
                    (A_NS, "graphic", 0),
                    (A_NS, "graphicData", 0),
                    (A_NS, "tbl", 0),
                ]);
                (part, path)
            }
            PptXmlOwner::Cell(part, path, row, column) => {
                steps.extend([
                    (A_NS, "graphic", 0),
                    (A_NS, "graphicData", 0),
                    (A_NS, "tbl", 0),
                    (A_NS, "tr", row),
                    (A_NS, "tc", column),
                ]);
                (part, path)
            }
            PptXmlOwner::TextBody(part, path, cell)
            | PptXmlOwner::Paragraph(part, path, cell, _)
            | PptXmlOwner::Run(part, path, cell, _, _) => {
                if let Some((row, column)) = cell {
                    steps.extend([
                        (A_NS, "graphic", 0),
                        (A_NS, "graphicData", 0),
                        (A_NS, "tbl", 0),
                        (A_NS, "tr", row),
                        (A_NS, "tc", column),
                        (A_NS, "txBody", 0),
                    ]);
                } else {
                    steps.push((P_NS, "txBody", 0));
                }
                match owner {
                    PptXmlOwner::Paragraph(_, _, _, paragraph) => {
                        steps.push((A_NS, "p", paragraph))
                    }
                    PptXmlOwner::Run(_, _, _, paragraph, run) => {
                        steps.extend([(A_NS, "p", paragraph), (A_NS, "r", run)])
                    }
                    _ => {}
                }
                (part, path)
            }
        };
        let id = self
            .part_shape_at(part, path)?
            .non_visual_id()
            .ok_or_else(|| {
                invalid_shape_mutation("replace XML", "physical shape identity is absent")
            })?;
        let (_, root) = PptXmlOwner::Shape(part, path).root(self)?;
        ppt_xml_owner_range(source, id, root, &steps)
    }

    /// Replace one shape-owned element when its relationship facts do not
    /// change. The edited child is validated on its own. A whole part, or a
    /// relationship change, keeps the package transaction.
    fn assign_local_owner_xml(
        &mut self,
        owner: PptXmlOwner<'_>,
        before: &[u8],
        xml: &str,
    ) -> Result<bool> {
        if matches!(owner, PptXmlOwner::Part(_)) {
            return Ok(false);
        }
        if ppt_xml_relationship_facts(before)? != ppt_xml_relationship_facts(xml.as_bytes())? {
            return Ok(false);
        }
        // Check the submitted bytes before typed projection can discard them.
        // A fragment cannot establish inherited MC authority, so use the
        // whole-part transaction when its local frontier cannot prove the edit.
        if check_ppt_frontier_changes(before, xml.as_bytes()).is_err() {
            return Ok(false);
        }
        if ppt_xml_known_owner_equivalent(before, xml.as_bytes())? {
            return Ok(true);
        }
        // A retained package signature has to observe the forced part bytes.
        // The local model write does not publish that part or the invalidation.
        if self
            .package
            .package_rels
            .get_by_type(rel_types::DIGITAL_SIGNATURE_ORIGIN)
            .is_some()
        {
            return Ok(false);
        }
        let part = owner.part();
        let path = match owner {
            PptXmlOwner::Shape(_, path)
            | PptXmlOwner::Table(_, path)
            | PptXmlOwner::Cell(_, path, _, _)
            | PptXmlOwner::TextBody(_, path, _)
            | PptXmlOwner::Paragraph(_, path, _, _)
            | PptXmlOwner::Run(_, path, _, _, _) => path.to_vec(),
            PptXmlOwner::Part(_) => return Ok(false),
        };
        let snapshot = child_at(&self.owner_data(part)?.shape_tree.children, &path)
            .ok_or_else(|| invalid_shape_mutation("replace XML", "shape owner is absent"))?
            .clone();
        let part_name = self.owner_part_name(part)?.to_owned();
        let assigned = (|| -> Result<()> {
            match owner {
                PptXmlOwner::Shape(_, shape_path) => {
                    self.assign_xml_shape(part, shape_path, xml)?
                }
                PptXmlOwner::Table(_, shape_path) => {
                    self.assign_xml_table(part, shape_path, xml)?
                }
                PptXmlOwner::Cell(_, shape_path, row, column) => {
                    self.assign_xml_cell(part, shape_path, row, column, xml)?
                }
                PptXmlOwner::TextBody(_, shape_path, cell) => {
                    self.assign_xml_body(part, shape_path, cell, xml)?
                }
                PptXmlOwner::Paragraph(_, shape_path, cell, index) => {
                    self.assign_xml_paragraph(part, shape_path, cell, index, xml)?
                }
                PptXmlOwner::Run(_, shape_path, cell, paragraph, index) => {
                    self.assign_xml_run(part, shape_path, cell, paragraph, index, xml)?
                }
                PptXmlOwner::Part(_) => {
                    return Err(invalid_shape_mutation(
                        "replace XML",
                        "a whole part is not a local owner",
                    ));
                }
            }
            let child = child_at(&self.owner_data(part)?.shape_tree.children, &path)
                .ok_or_else(|| invalid_shape_mutation("replace XML", "shape owner is absent"))?;
            validate_shape_child(child, &part_name)?;
            let after = self.checked_owner_xml(owner)?;
            check_ppt_frontier_changes(before, &after)?;
            Ok(())
        })();
        if let Err(error) = assigned {
            if let Some(child) =
                child_at_mut(&mut self.owner_data_mut(part)?.shape_tree.children, &path)
            {
                *child = snapshot;
            }
            return Err(error);
        }
        Ok(true)
    }

    fn replace_checked_owner_xml(&mut self, owner: PptXmlOwner<'_>, xml: &str) -> Result<()> {
        let before = self.checked_owner_xml(owner)?;
        if before == xml.as_bytes() {
            return Ok(());
        }
        let (namespace, local) = owner.root(self)?;
        check_ppt_xml_input(xml.as_bytes(), namespace, local)?;
        if self.assign_local_owner_xml(owner, &before, xml)? {
            return Ok(());
        }
        let before_part = self.checked_owner_xml(PptXmlOwner::Part(owner.part()))?;
        self.check_xml_relationships(owner.part(), &before, xml.as_bytes())?;
        self.validate_xml_part_model(owner.part())?;
        let range = self.xml_physical_owner_range(owner, &before_part)?;
        let mut raw_candidate = before_part.clone();
        raw_candidate.splice(range, xml.as_bytes().iter().copied());
        check_ppt_frontier_changes(&before_part, &raw_candidate)?;
        if ppt_xml_known_owner_equivalent(&before, xml.as_bytes())? {
            return Ok(());
        }
        let mut candidate = self.clone();
        match owner {
            PptXmlOwner::Part(ShapeOwner::Slide(index)) => {
                candidate.assign_xml_slide(index, xml)?
            }
            PptXmlOwner::Part(ShapeOwner::Layout(index)) => {
                candidate.assign_xml_layout(index, xml)?
            }
            PptXmlOwner::Part(ShapeOwner::Master(index)) => {
                candidate.assign_xml_master(index, xml)?
            }
            PptXmlOwner::Shape(part, path) => candidate.assign_xml_shape(part, path, xml)?,
            PptXmlOwner::Table(part, path) => candidate.assign_xml_table(part, path, xml)?,
            PptXmlOwner::Cell(part, path, row, column) => {
                candidate.assign_xml_cell(part, path, row, column, xml)?
            }
            PptXmlOwner::TextBody(part, path, cell) => {
                candidate.assign_xml_body(part, path, cell, xml)?
            }
            PptXmlOwner::Paragraph(part, path, cell, index) => {
                candidate.assign_xml_paragraph(part, path, cell, index, xml)?
            }
            PptXmlOwner::Run(part, path, cell, paragraph, index) => {
                candidate.assign_xml_run(part, path, cell, paragraph, index, xml)?
            }
        }
        let after_part = candidate.checked_owner_xml(PptXmlOwner::Part(owner.part()))?;
        check_ppt_frontier_changes(&before_part, &after_part)?;
        candidate.validate_xml_part_model(owner.part())?;
        // Compare the concrete modeled owner as well as retained source payload.
        // Namespace spelling alone must not publish a needless mutation.
        if candidate.checked_owner_xml(owner)? == before {
            return Ok(());
        }
        let previous = relationship_ids(&before)
            .map_err(|error| invalid_shape_mutation("replace XML", error.to_string()))?
            .into_iter()
            .collect::<HashSet<_>>();
        let referenced = candidate.owner_relationship_ids(owner.part())?;
        let part = candidate.owner_part_name(owner.part())?.to_owned();
        if let Some(relationships) = candidate.package.get_part_rels(&part).cloned() {
            let mut released = HashSet::new();
            let retained = relationships
                .items
                .iter()
                .filter(|relationship| {
                    let superseded = previous.contains(&relationship.id)
                        && !referenced.contains(&relationship.id)
                        && relationship_requires_xml_reference(&relationship.rel_type);
                    if superseded && !relationship_is_external(relationship) {
                        released
                            .insert(OpcPackage::resolve_rel_target(&part, &relationship.target));
                    }
                    !superseded
                })
                .cloned()
                .collect::<Vec<_>>();
            if retained.len() != relationships.items.len() {
                let retained = relationships.rebind_for_import(retained, &HashMap::new())?;
                candidate.package.set_part_rels(&part, retained);
                prune_unreachable_parts(&mut candidate.package, &released);
                candidate.media_store = candidate.media_store.rescan(&candidate.package);
            }
        }
        candidate.commit_owned_parts(Some(owner.part()))?;
        *self = candidate;
        Ok(())
    }

    #[inline(never)]
    fn assign_xml_shape(&mut self, part: ShapeOwner, path: &[usize], xml: &str) -> Result<()> {
        let parsed_error =
            |error: OxmlError| invalid_shape_mutation("replace XML", error.to_string());

        let old = child_at(&self.owner_data(part)?.shape_tree.children, path)
            .ok_or_else(|| invalid_shape_mutation("replace XML", "shape owner is absent"))?;
        let parsed = match old {
            ShapeTreeChild::Shape(_) => {
                ShapeTreeChild::Shape(CT_Shape::from_xml(xml.as_bytes()).map_err(parsed_error)?)
            }
            ShapeTreeChild::Picture(_) => {
                ShapeTreeChild::Picture(CT_Picture::from_xml(xml.as_bytes()).map_err(parsed_error)?)
            }
            ShapeTreeChild::GraphicFrame(_) => ShapeTreeChild::GraphicFrame(Box::new(
                CT_GraphicFrame::from_xml(xml.as_bytes()).map_err(parsed_error)?,
            )),
            ShapeTreeChild::GroupShape(_) => ShapeTreeChild::GroupShape(Box::new(
                CT_GroupShape::from_xml(xml.as_bytes()).map_err(parsed_error)?,
            )),
            ShapeTreeChild::Connector(_) => ShapeTreeChild::Connector(
                CT_ConnectionShape::from_xml(xml.as_bytes()).map_err(parsed_error)?,
            ),
            ShapeTreeChild::AlternateContent(_) => {
                return Err(invalid_shape_mutation(
                    "replace XML",
                    "raw alternate shape owner is read only",
                ));
            }
        };
        if old.non_visual_id() != parsed.non_visual_id() {
            return Err(invalid_shape_mutation(
                "replace XML",
                "shape identity cannot change",
            ));
        }
        *child_at_mut(&mut self.owner_data_mut(part)?.shape_tree.children, path)
            .ok_or_else(|| invalid_shape_mutation("replace XML", "shape owner is absent"))? =
            parsed;
        Ok(())
    }

    #[inline(never)]
    fn assign_xml_table(&mut self, part: ShapeOwner, path: &[usize], xml: &str) -> Result<()> {
        let parsed_error =
            |error: OxmlError| invalid_shape_mutation("replace XML", error.to_string());

        let parsed = CT_Table::from_xml(xml.as_bytes()).map_err(parsed_error)?;
        parsed.validate_grid_and_merges().map_err(parsed_error)?;
        *ppt_xml_table_mut(self.xml_child_mut(part, path)?)? = parsed;
        Ok(())
    }

    #[inline(never)]
    fn assign_xml_cell(
        &mut self,
        part: ShapeOwner,
        path: &[usize],
        row: usize,
        column: usize,
        xml: &str,
    ) -> Result<()> {
        let parsed_error =
            |error: OxmlError| invalid_shape_mutation("replace XML", error.to_string());

        let parsed = CT_TableCell::from_xml_fragment(xml.as_bytes()).map_err(parsed_error)?;
        let table = ppt_xml_table_mut(self.xml_child_mut(part, path)?)?;
        *table
            .rows
            .get_mut(row)
            .and_then(|row| row.cells.get_mut(column))
            .ok_or_else(|| invalid_shape_mutation("replace XML", "cell owner is absent"))? = parsed;
        table.validate_grid_and_merges().map_err(parsed_error)?;
        Ok(())
    }

    #[inline(never)]
    fn assign_xml_body(
        &mut self,
        part: ShapeOwner,
        path: &[usize],
        cell: Option<(usize, usize)>,
        xml: &str,
    ) -> Result<()> {
        let parsed = CT_TextBody::from_xml(xml.as_bytes())
            .map_err(|error| invalid_shape_mutation("replace XML", error.to_string()))?;
        *ppt_xml_body_mut(self.xml_child_mut(part, path)?, cell)? = parsed;
        Ok(())
    }

    #[inline(never)]
    fn assign_xml_paragraph(
        &mut self,
        part: ShapeOwner,
        path: &[usize],
        cell: Option<(usize, usize)>,
        index: usize,
        xml: &str,
    ) -> Result<()> {
        let parsed = CT_TextParagraph::from_xml(xml.as_bytes())
            .map_err(|error| invalid_shape_mutation("replace XML", error.to_string()))?;
        *ppt_xml_body_mut(self.xml_child_mut(part, path)?, cell)?
            .paragraph_mut(index)
            .ok_or_else(|| invalid_shape_mutation("replace XML", "paragraph owner is absent"))? =
            parsed;
        Ok(())
    }

    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn assign_xml_run(
        &mut self,
        part: ShapeOwner,
        path: &[usize],
        cell: Option<(usize, usize)>,
        paragraph: usize,
        index: usize,
        xml: &str,
    ) -> Result<()> {
        let parsed = CT_RegularTextRun::from_xml(xml.as_bytes())
            .map_err(|error| invalid_shape_mutation("replace XML", error.to_string()))?;
        let paragraph = ppt_xml_body_mut(self.xml_child_mut(part, path)?, cell)?
            .paragraph_mut(paragraph)
            .ok_or_else(|| invalid_shape_mutation("replace XML", "paragraph owner is absent"))?;
        *paragraph
            .runs
            .iter_mut()
            .filter(|run| matches!(run, TextRun::Run(_)))
            .nth(index)
            .ok_or_else(|| {
                invalid_shape_mutation("replace XML", "regular run owner is absent")
            })? = TextRun::Run(parsed);
        Ok(())
    }

    #[inline(never)]
    fn assign_xml_slide(&mut self, index: usize, xml: &str) -> Result<()> {
        Arc::make_mut(&mut self.slides[index]).slide = CT_Slide::from_xml(xml.as_bytes())
            .map_err(|error| invalid_shape_mutation("replace XML", error.to_string()))?;
        Ok(())
    }
    #[inline(never)]
    fn assign_xml_layout(&mut self, index: usize, xml: &str) -> Result<()> {
        Arc::make_mut(&mut self.layouts[index]).layout =
            CT_SlideLayout::from_xml(xml.as_bytes())
                .map_err(|error| invalid_shape_mutation("replace XML", error.to_string()))?;
        Arc::make_mut(&mut self.layouts[index]).dirty = true;
        Ok(())
    }
    #[inline(never)]
    fn assign_xml_master(&mut self, index: usize, xml: &str) -> Result<()> {
        Arc::make_mut(&mut self.masters[index]).master =
            Ok(CT_SlideMaster::from_xml(xml.as_bytes())
                .map_err(|error| invalid_shape_mutation("replace XML", error.to_string()))?);
        Arc::make_mut(&mut self.masters[index]).dirty = true;
        Ok(())
    }

    fn xml_child_mut(&mut self, part: ShapeOwner, path: &[usize]) -> Result<&mut ShapeTreeChild> {
        child_at_mut(&mut self.owner_data_mut(part)?.shape_tree.children, path)
            .ok_or_else(|| invalid_shape_mutation("replace XML", "physical shape owner is absent"))
    }

    fn validate_xml_part_model(&self, part: ShapeOwner) -> Result<()> {
        let data = self.owner_data(part)?;
        oxoffice_pptx_oxml::shape_tree::validate_shape_nesting(&data.shape_tree.children)
            .map_err(|error| invalid_shape_mutation("replace XML", error.to_string()))?;
        let mut ids = HashSet::new();
        if let Some(id) = data.shape_tree.non_visual_id() {
            ids.insert(id);
        }
        let mut pending = data.shape_tree.children.iter().collect::<Vec<_>>();
        while let Some(child) = pending.pop() {
            if child
                .non_visual_id()
                .is_some_and(|id| id == 0 || !ids.insert(id))
            {
                return Err(invalid_shape_mutation(
                    "replace XML",
                    "duplicate or zero shape identity",
                ));
            }
            match child {
                ShapeTreeChild::GroupShape(group) => pending.extend(group.children.iter()),
                ShapeTreeChild::GraphicFrame(frame) => {
                    if let GraphicDataPayload::Table(table) = frame.graphic_data.payload() {
                        table.validate_grid_and_merges().map_err(|error| {
                            invalid_shape_mutation("replace XML", error.to_string())
                        })?;
                    }
                }
                ShapeTreeChild::AlternateContent(alternate) => {
                    if let Some(children) = alternate.selected_fallback() {
                        pending.extend(children);
                    }
                }
                _ => {}
            }
        }
        self.validate_owned_part(part)
    }

    fn check_xml_relationships(
        &self,
        part: ShapeOwner,
        before: &[u8],
        incoming: &[u8],
    ) -> Result<()> {
        let package = self.staged_package(true)?;
        let source = self.owner_part_name(part)?;
        let relations = package.get_part_rels(source);
        let old = ppt_xml_relationship_facts(before)?;
        for fact in ppt_xml_relationship_facts(incoming)? {
            // The existing slide-removal provider retains its actual cleared
            // click record without a relationship. Preserve only that source
            // fact, never treat an arbitrary new empty ID as a relationship.
            if fact.0 == A_NS
                && matches!(
                    fact.1.as_str(),
                    "hlinkClick" | "hlinkMouseOver" | "hlinkHover"
                )
                && fact.2 == "id"
                && fact.3.is_empty()
                && fact.4.as_deref() == Some("ppaction://noaction")
                && old.contains(&fact)
            {
                continue;
            }
            let relation = relations
                .and_then(|relations| relations.get_by_id(&fact.3))
                .ok_or_else(|| {
                    invalid_shape_mutation(
                        "replace XML",
                        format!("relationship {} is absent from owner part {source}", fact.3),
                    )
                })?;
            let expected = match (fact.0.as_str(), fact.1.as_str(), fact.2.as_str()) {
                (A_NS, "hlinkClick" | "hlinkMouseOver", "id") => {
                    Some(if fact.4.as_deref() == Some("ppaction://hlinksldjump") {
                        &[rel_types::SLIDE][..]
                    } else {
                        &[rel_types::HYPERLINK][..]
                    })
                }
                (A_NS, "blip", "embed" | "link") => Some(&[rel_types::IMAGE][..]),
                (A_NS, "videoFile", "link") => Some(&[rel_types::VIDEO][..]),
                (A_NS, "audioFile" | "wavAudioFile", "link" | "embed") => {
                    Some(&[rel_types::AUDIO][..])
                }
                ("http://schemas.openxmlformats.org/drawingml/2006/chart", "chart", "id") => {
                    Some(&[rel_types::CHART][..])
                }
                _ => None,
            };
            if expected.is_some_and(|types| !types.contains(&relation.rel_type.as_str()))
                || (expected.is_none() && !old.contains(&fact))
            {
                return Err(invalid_shape_mutation(
                    "replace XML",
                    "relationship type or opaque source role is unsupported",
                ));
            }
            let external = relationship_is_external(relation);
            if relation.rel_type == rel_types::HYPERLINK {
                if !external || relation.target.is_empty() {
                    return Err(invalid_shape_mutation(
                        "replace XML",
                        "hyperlink relationship requires a nonempty external target",
                    ));
                }
                continue;
            }
            if external {
                if fact.2 == "embed"
                    || matches!(
                        relation.rel_type.as_str(),
                        rel_types::SLIDE | rel_types::CHART
                    )
                {
                    return Err(invalid_shape_mutation(
                        "replace XML",
                        "embedded media, slide and chart relationships require internal targets",
                    ));
                }
                continue;
            }
            let target = if let Some(types) = expected {
                require_internal_related_part(&package, source, &relation.id, types[0])?
            } else {
                OpcPackage::resolve_rel_target(source, &relation.target)
            };
            let bytes = required_part(&package, &target)?;
            let content_type = package.content_types.content_type_for(&target);
            match relation.rel_type.as_str() {
                rel_types::SLIDE => {
                    if content_type != Some(content_types::SLIDE) {
                        return Err(invalid_shape_mutation(
                            "replace XML",
                            "slide relationship requires actual slide content type",
                        ));
                    }
                    check_ppt_xml_input(bytes, P_NS, b"sld")?;
                    CT_Slide::from_xml(bytes).map_err(|error| {
                        invalid_shape_mutation("replace XML", error.to_string())
                    })?;
                }
                rel_types::IMAGE => {
                    let format = oxoffice_media::ImageFormat::sniff(bytes).ok_or_else(|| {
                        invalid_shape_mutation(
                            "replace XML",
                            "image target format is not recognized",
                        )
                    })?;
                    if content_type != Some(format.content_type()) {
                        return Err(invalid_shape_mutation(
                            "replace XML",
                            "image target content type does not match its actual format",
                        ));
                    }
                    if matches!(
                        format,
                        oxoffice_media::ImageFormat::Png
                            | oxoffice_media::ImageFormat::Jpeg
                            | oxoffice_media::ImageFormat::Gif
                            | oxoffice_media::ImageFormat::Tiff
                            | oxoffice_media::ImageFormat::Bmp
                    ) {
                        validate_semantic_picture(bytes, format)
                            .map_err(|message| invalid_shape_mutation("replace XML", message))?;
                    } else {
                        // Unsupported original media stays opaque only when the
                        // selected owner's exact role and original owned target
                        // survive. This is source preservation, not decode proof.
                        let original = self
                            .package
                            .get_part_rels(source)
                            .and_then(|relations| relations.get_by_id(&relation.id));
                        let unchanged = old.contains(&fact)
                            && original.is_some_and(|original| {
                                original.rel_type == relation.rel_type
                                    && original.target == relation.target
                                    && original.target_mode == relation.target_mode
                            })
                            && self.package.get_part(&target) == Some(bytes)
                            && self.package.content_types.content_type_for(&target) == content_type;
                        if !unchanged {
                            return Err(invalid_shape_mutation(
                                "replace XML",
                                "new unsupported image format cannot be checked, only an exact original owned media fact may be preserved",
                            ));
                        }
                    }
                }
                rel_types::CHART => {
                    if content_type != Some(content_types::CHART) {
                        return Err(invalid_shape_mutation(
                            "replace XML",
                            "chart relationship requires actual chart content type",
                        ));
                    }
                    check_ppt_xml_input(
                        bytes,
                        "http://schemas.openxmlformats.org/drawingml/2006/chart",
                        b"chartSpace",
                    )?;
                    CT_ChartSpace::from_xml(bytes).map_err(|error| {
                        invalid_shape_mutation("replace XML", error.to_string())
                    })?;
                }
                rel_types::AUDIO | rel_types::VIDEO => {
                    let Some(content_type) = content_type else {
                        return Err(invalid_shape_mutation(
                            "replace XML",
                            "media target has no content type",
                        ));
                    };
                    let prefix = if relation.rel_type == rel_types::AUDIO {
                        "audio/"
                    } else {
                        "video/"
                    };
                    if !content_type.starts_with(prefix)
                        || oxoffice_media::audio_video_signature_matches(bytes, content_type)
                            != Some(true)
                    {
                        return Err(invalid_shape_mutation(
                            "replace XML",
                            "media relationship requires supported actual content and mode",
                        ));
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub(super) fn validate_owned_part(&self, owner: ShapeOwner) -> Result<()> {
        let result = match owner {
            ShapeOwner::Slide(index) => self.slides[index].slide.to_xml(),
            ShapeOwner::Layout(index) => self.layouts[index].layout.to_xml(),
            ShapeOwner::Master(index) => self.masters[index]
                .master
                .as_ref()
                .map_err(|message| Error::MalformedPart {
                    part_name: self.masters[index].part_name.clone(),
                    message: message.clone(),
                })?
                .to_xml(),
        };
        result.map(|_| ()).map_err(|error| Error::MalformedPart {
            part_name: self.owner_part_name(owner).unwrap_or_default().to_owned(),
            message: error.to_string(),
        })
    }
}

//! Bounded PDF page import into the PresentationML facade.

use std::collections::{BTreeMap, HashMap, HashSet};
#[cfg(not(target_arch = "wasm32"))]
use std::io::{Read, Take};
#[cfg(not(target_arch = "wasm32"))]
use std::path::Path;
use std::sync::Arc;

use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, LoadOptions, Object, ObjectId, Stream};
use oxoffice_core::units::{Angle, Emu};
use oxoffice_drawing::fill::Fill;
use oxoffice_drawing::geometry::CT_CustomGeometry2D;
use oxoffice_drawing::line::CT_LineProperties;
use oxoffice_drawing::text::{CT_TextCharacterProperties, Coordinate32Value, TextFont};
use oxoffice_layout::{
    Color, FillRule, FontFile, FontManager, GroupElement, LayoutResult, LineCap, LineJoin, MediaId,
    PageFrame, Paint, Path as LayoutPath, PathCommand, PathElement, Point, PositionedElement, Rect,
    Stroke, Transform,
};

use super::*;

const EMU_PER_POINT: f64 = 12_700.0;
const MAX_PDF_GRAPH_DEPTH: usize = 128;

/// Selects the bounded PDF projection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PdfImportMode {
    /// Render supported page content through the shared rasterizer and insert one picture.
    PreservedGraphic { dpi: f64 },
    /// Project supported text, images, paths, and links as editable slide content.
    Editable,
}

/// Hard resource limits for one PDF import.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PdfImportLimits {
    pub max_input_bytes: u64,
    pub max_pages: usize,
    pub max_objects: usize,
    pub max_decompressed_bytes: usize,
    pub max_operations_per_page: usize,
    pub max_pixels_per_page: u64,
    pub max_shapes_per_page: usize,
    pub max_diagnostics: usize,
}

impl Default for PdfImportLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 64 * 1024 * 1024,
            max_pages: 256,
            max_objects: 250_000,
            max_decompressed_bytes: 128 * 1024 * 1024,
            max_operations_per_page: 1_000_000,
            max_pixels_per_page: 100_000_000,
            max_shapes_per_page: 100_000,
            max_diagnostics: 10_000,
        }
    }
}

/// One stable source-path diagnostic for unsupported but safely skippable content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PdfImportDiagnostic {
    pub path: String,
    pub message: String,
}

/// A complete transactional PDF import.
pub struct PdfImportResult {
    pub presentation: Presentation,
    pub diagnostics: Vec<PdfImportDiagnostic>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PageGeometry {
    left: f64,
    bottom: f64,
    right: f64,
    top: f64,
    rotation: i32,
    width: f64,
    height: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Matrix {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
}

impl Matrix {
    const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    fn then(self, next: Self) -> Self {
        Self {
            a: next.a * self.a + next.c * self.b,
            b: next.b * self.a + next.d * self.b,
            c: next.a * self.c + next.c * self.d,
            d: next.b * self.c + next.d * self.d,
            e: next.a * self.e + next.c * self.f + next.e,
            f: next.b * self.e + next.d * self.f + next.f,
        }
    }

    fn point(self, x: f64, y: f64) -> Point {
        Point {
            x: self.a * x + self.c * y + self.e,
            y: self.b * x + self.d * y + self.f,
        }
    }

    fn translated(self, x: f64, y: f64) -> Self {
        Self {
            e: self.e + self.a * x + self.c * y,
            f: self.f + self.b * x + self.d * y,
            ..self
        }
    }
}

impl PageGeometry {
    fn normalize(self, point: Point) -> Point {
        match self.rotation {
            0 => Point {
                x: point.x - self.left,
                y: self.top - point.y,
            },
            90 => Point {
                x: point.y - self.bottom,
                y: point.x - self.left,
            },
            180 => Point {
                x: self.right - point.x,
                y: point.y - self.bottom,
            },
            270 => Point {
                x: self.top - point.y,
                y: self.right - point.x,
            },
            _ => unreachable!("rotation is normalized"),
        }
    }

    fn rect(self, values: [f64; 4]) -> Rect {
        let first = self.normalize(Point {
            x: values[0],
            y: values[1],
        });
        let second = self.normalize(Point {
            x: values[2],
            y: values[3],
        });
        Rect {
            x: first.x.min(second.x),
            y: first.y.min(second.y),
            width: (first.x - second.x).abs(),
            height: (first.y - second.y).abs(),
        }
    }
}

#[derive(Clone)]
enum EditableElement {
    Text {
        rect: Rect,
        transform: Transform,
        text: String,
        family: String,
        size: f64,
        color: Color,
    },
    Path(PathElement),
    Image {
        rect: Rect,
        transform: Transform,
        data: Vec<u8>,
        filename: String,
    },
}

struct NormalizedPage {
    number: u32,
    geometry: PageGeometry,
    layout: Vec<PositionedElement>,
    editable: Vec<EditableElement>,
    links: Vec<(Rect, String)>,
    image_pixels: u64,
    retained_elements: usize,
}

#[derive(Clone)]
struct PageFont {
    display_family: String,
    shaping_family: String,
    encoding: String,
    encoding_supported: bool,
    decoded: BTreeMap<Vec<u8>, String>,
    widths: Option<SimpleFontWidths>,
    composite_widths_supported: bool,
}

#[derive(Clone)]
struct SimpleFontWidths {
    first_char: i64,
    widths: Vec<f64>,
    missing_width: f64,
}

struct EmbeddedFonts {
    files: Vec<FontFile>,
    retained_bytes: usize,
    widths: HashMap<ObjectId, Option<SimpleFontWidths>>,
}

enum CachedFontEncoding {
    Unicode(lopdf::Encoding<'static>),
    SingleByte(Vec<String>),
}

#[derive(Default)]
struct FontEncodingCaches {
    to_unicode_by_stream: HashMap<ObjectId, Arc<CachedFontEncoding>>,
    ordinary_by_font: HashMap<ObjectId, Arc<CachedFontEncoding>>,
    to_unicode_parses: usize,
}

impl CachedFontEncoding {
    fn decode(&self, bytes: &[u8]) -> String {
        match self {
            Self::SingleByte(values) => bytes
                .iter()
                .filter_map(|byte| values.get(*byte as usize))
                .cloned()
                .collect(),
            Self::Unicode(encoding) => Document::decode_text(encoding, bytes)
                .unwrap_or_else(|_| String::from_utf8_lossy(bytes).into_owned()),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct DashState {
    pattern: Vec<f64>,
    phase: f64,
}

#[derive(Clone)]
struct GraphicsState {
    ctm: Matrix,
    fill: Color,
    stroke: Color,
    line_width: f64,
    line_cap: LineCap,
    line_join: LineJoin,
    dash: Option<DashState>,
    text_matrix: Matrix,
    line_matrix: Matrix,
    font_resource: Option<Vec<u8>>,
    font_size: f64,
    character_spacing: f64,
    word_spacing: f64,
    horizontal_scale: f64,
    leading: f64,
    rise: f64,
    in_text: bool,
    text_visible: bool,
    text_metrics_supported: bool,
    text_position_supported: bool,
    fill_supported: bool,
    stroke_supported: bool,
    dash_supported: bool,
    graphics_supported: bool,
}

impl Default for GraphicsState {
    fn default() -> Self {
        Self {
            ctm: Matrix::IDENTITY,
            fill: Color::BLACK,
            stroke: Color::BLACK,
            line_width: 1.0,
            line_cap: LineCap::Butt,
            line_join: LineJoin::Miter,
            dash: None,
            text_matrix: Matrix::IDENTITY,
            line_matrix: Matrix::IDENTITY,
            font_resource: None,
            font_size: 12.0,
            character_spacing: 0.0,
            word_spacing: 0.0,
            horizontal_scale: 1.0,
            leading: 0.0,
            rise: 0.0,
            in_text: false,
            text_visible: true,
            text_metrics_supported: true,
            text_position_supported: true,
            fill_supported: true,
            stroke_supported: true,
            dash_supported: true,
            graphics_supported: true,
        }
    }
}

struct Importer<'a> {
    document: Document,
    limits: PdfImportLimits,
    mode: PdfImportMode,
    diagnostics: Vec<PdfImportDiagnostic>,
    decompressed: usize,
    fonts: FontManager,
    embedded_font_aliases: HashSet<String>,
    encoding_caches: FontEncodingCaches,
    font_widths: HashMap<ObjectId, Option<SimpleFontWidths>>,
    _bytes: &'a [u8],
}

impl Presentation {
    /// Imports one complete PDF byte slice with default hard limits.
    pub fn from_pdf_bytes(bytes: &[u8], mode: PdfImportMode) -> Result<PdfImportResult> {
        Self::from_pdf_bytes_with_limits(bytes, mode, PdfImportLimits::default())
    }

    /// Imports one complete PDF byte slice with caller-selected hard limits.
    pub fn from_pdf_bytes_with_limits(
        bytes: &[u8],
        mode: PdfImportMode,
        limits: PdfImportLimits,
    ) -> Result<PdfImportResult> {
        validate_limits(bytes, mode, limits)?;
        let options = LoadOptions {
            strict: true,
            max_decompressed_size: Some(limits.max_decompressed_bytes),
            ..LoadOptions::default()
        };
        let document = Document::load_mem_with_options(bytes, options).map_err(|error| {
            import_error(None, None, format!("strict PDF parse failed: {error}"))
        })?;
        if document.trailer.has(b"Encrypt") {
            return Err(import_error(
                None,
                None,
                "encrypted PDF input is unsupported",
            ));
        }
        if document.objects.len() > limits.max_objects {
            return Err(import_error(
                None,
                None,
                format!(
                    "PDF object count {} exceeds limit {}",
                    document.objects.len(),
                    limits.max_objects
                ),
            ));
        }
        reject_active_content(&document, limits.max_objects)?;
        let fonts = FontManager::new_deterministic().map_err(|error| {
            import_error(
                None,
                None,
                format!("deterministic font manager failed: {error}"),
            )
        })?;
        Importer {
            document,
            limits,
            mode,
            diagnostics: Vec::new(),
            decompressed: 0,
            fonts,
            embedded_font_aliases: HashSet::new(),
            encoding_caches: FontEncodingCaches::default(),
            font_widths: HashMap::new(),
            _bytes: bytes,
        }
        .import()
    }

    /// Opens and imports one PDF path without reading beyond the input limit.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_pdf<P: AsRef<Path>>(path: P, mode: PdfImportMode) -> Result<PdfImportResult> {
        let limits = PdfImportLimits::default();
        let file = std::fs::File::open(path).map_err(OpcError::from)?;
        let length = file.metadata().map_err(OpcError::from)?.len();
        if length > limits.max_input_bytes {
            return Err(import_error(
                None,
                None,
                format!(
                    "PDF input length {length} exceeds limit {}",
                    limits.max_input_bytes
                ),
            ));
        }
        let mut reader: Take<_> = file.take(limits.max_input_bytes.saturating_add(1));
        let mut bytes = Vec::with_capacity(usize::try_from(length).unwrap_or(0));
        reader.read_to_end(&mut bytes).map_err(OpcError::from)?;
        Self::from_pdf_bytes_with_limits(&bytes, mode, limits)
    }
}

impl Importer<'_> {
    fn import(mut self) -> Result<PdfImportResult> {
        let pages = strict_pages(
            &self.document,
            self.limits.max_pages,
            self.limits.max_objects,
        )?;
        for &(number, id) in &pages {
            page_geometry(&self.document, id, number)?;
        }
        let page_resources = pages
            .iter()
            .map(|&(number, id)| {
                resolve_page_resources(&self.document, id, number, self.limits.max_objects)
            })
            .collect::<Result<Vec<_>>>()?;

        let embedded = embedded_fonts(
            &mut self.document,
            page_resources.iter().map(|resources| &resources.fonts),
            self.limits.max_decompressed_bytes,
            self.limits.max_objects,
        )?;
        self.decompressed = embedded.retained_bytes;
        self.embedded_font_aliases = embedded
            .files
            .iter()
            .map(|font| font.family.clone())
            .collect();
        self.font_widths = embedded.widths;
        self.fonts.load_additional_fonts(&embedded.files);
        let mut normalized = Vec::with_capacity(pages.len());
        for ((number, id), resources) in pages.into_iter().zip(&page_resources) {
            normalized.push(self.parse_page(number, id, resources)?);
        }
        let first = normalized[0].geometry;
        for page in normalized.iter().skip(1) {
            if (page.geometry.width - first.width).abs() > 0.000_001
                || (page.geometry.height - first.height).abs() > 0.000_001
            {
                return Err(import_error(
                    Some(page.number),
                    None,
                    format!(
                        "effective page size {} by {} differs from first page {} by {}",
                        page.geometry.width, page.geometry.height, first.width, first.height
                    ),
                ));
            }
        }
        let presentation = self.project(&normalized)?;
        Ok(PdfImportResult {
            presentation,
            diagnostics: self.diagnostics,
        })
    }

    fn parse_page(
        &mut self,
        number: u32,
        id: ObjectId,
        resources: &ResolvedPageResources,
    ) -> Result<NormalizedPage> {
        let geometry = page_geometry(&self.document, id, number)?;
        let remaining = self.remaining_decompressed(Some(number), None)?;
        let data = strict_page_content(
            &self.document,
            id,
            number,
            self.limits.max_objects,
            remaining,
        )?;
        self.charge_decompressed(data.len(), Some(number), None, "page content")?;
        let sanitized = sanitize_content_comments(&data);
        let content = Content::decode_strict(&sanitized).map_err(|error| {
            import_error(
                Some(number),
                None,
                format!("strict content decode failed: {error}"),
            )
        })?;
        if content.operations.len() > self.limits.max_operations_per_page {
            return Err(import_error(
                Some(number),
                None,
                format!(
                    "page operation count {} exceeds limit {}",
                    content.operations.len(),
                    self.limits.max_operations_per_page
                ),
            ));
        }
        let fonts = page_fonts(
            &self.document,
            &mut self.encoding_caches,
            &resources.fonts,
            &content.operations,
            &self.font_widths,
        )?;
        let mut page = NormalizedPage {
            number,
            geometry,
            layout: Vec::new(),
            editable: Vec::new(),
            links: Vec::new(),
            image_pixels: 0,
            retained_elements: 0,
        };
        self.interpret(&mut page, &content.operations, &fonts, &resources.xobjects)?;
        self.parse_annotations(&mut page, id)?;
        if page.retained_elements > self.limits.max_shapes_per_page {
            return Err(import_error(
                Some(number),
                None,
                format!(
                    "page shape count exceeds limit {}",
                    self.limits.max_shapes_per_page
                ),
            ));
        }
        Ok(page)
    }

    fn diagnostic(&mut self, page: u32, offset: usize, message: impl Into<String>) -> Result<()> {
        if self.diagnostics.len() >= self.limits.max_diagnostics {
            return Err(import_error(
                Some(page),
                Some(offset),
                format!(
                    "diagnostic count exceeds limit {}",
                    self.limits.max_diagnostics
                ),
            ));
        }
        self.diagnostics.push(PdfImportDiagnostic {
            path: format!("pdf/page[{page}]/content/op[{offset}]"),
            message: message.into(),
        });
        Ok(())
    }

    fn remaining_decompressed(&self, page: Option<u32>, offset: Option<usize>) -> Result<usize> {
        let remaining = self
            .limits
            .max_decompressed_bytes
            .checked_sub(self.decompressed)
            .ok_or_else(|| {
                import_error(
                    page,
                    offset,
                    "aggregate decompressed byte budget is exhausted",
                )
            })?;
        if remaining == 0 {
            return Err(import_error(
                page,
                offset,
                "aggregate decompressed byte budget is exhausted",
            ));
        }
        Ok(remaining)
    }

    fn charge_decompressed(
        &mut self,
        bytes: usize,
        page: Option<u32>,
        offset: Option<usize>,
        source: &str,
    ) -> Result<()> {
        self.decompressed = self.decompressed.checked_add(bytes).ok_or_else(|| {
            import_error(page, offset, "aggregate decompressed byte count overflow")
        })?;
        if self.decompressed > self.limits.max_decompressed_bytes {
            return Err(import_error(
                page,
                offset,
                format!(
                    "aggregate decompressed bytes after {source} total {}, exceeding limit {}",
                    self.decompressed, self.limits.max_decompressed_bytes
                ),
            ));
        }
        Ok(())
    }

    fn interpret(
        &mut self,
        page: &mut NormalizedPage,
        operations: &[Operation],
        fonts: &BTreeMap<Vec<u8>, PageFont>,
        xobjects: &BTreeMap<Vec<u8>, ObjectId>,
    ) -> Result<()> {
        let mut state = GraphicsState::default();
        let mut stack = Vec::new();
        let mut path = Vec::<PathCommand>::new();
        let mut current: Option<Point> = None;
        let mut subpath: Option<Point> = None;

        for (offset, operation) in operations.iter().enumerate() {
            validate_operator_arity(operation, page.number, offset)?;
            match operation.operator.as_str() {
                "q" => stack.push(state.clone()),
                "Q" => {
                    state = stack.pop().ok_or_else(|| {
                        import_error(Some(page.number), Some(offset), "unbalanced Q operator")
                    })?;
                }
                "cm" => {
                    let values = numbers(operation, 6, page.number, offset)?;
                    let next = Matrix {
                        a: values[0],
                        b: values[1],
                        c: values[2],
                        d: values[3],
                        e: values[4],
                        f: values[5],
                    };
                    state.ctm = checked_matrix(
                        next.then(state.ctm),
                        page.number,
                        offset,
                        "concatenated CTM",
                    )?;
                }
                "m" => {
                    let p = transformed_point(
                        operation,
                        &state,
                        page.geometry,
                        page.number,
                        offset,
                        0,
                    )?;
                    path.push(PathCommand::MoveTo(p));
                    current = Some(p);
                    subpath = Some(p);
                }
                "l" => {
                    require_current_point(current, page.number, offset, "l")?;
                    let p = transformed_point(
                        operation,
                        &state,
                        page.geometry,
                        page.number,
                        offset,
                        0,
                    )?;
                    path.push(PathCommand::LineTo(p));
                    current = Some(p);
                }
                "c" => {
                    require_current_point(current, page.number, offset, "c")?;
                    let values = numbers(operation, 6, page.number, offset)?;
                    let c1 = checked_point(
                        page.geometry
                            .normalize(state.ctm.point(values[0], values[1])),
                        page.number,
                        offset,
                        "curve control point",
                    )?;
                    let c2 = checked_point(
                        page.geometry
                            .normalize(state.ctm.point(values[2], values[3])),
                        page.number,
                        offset,
                        "curve control point",
                    )?;
                    let to = checked_point(
                        page.geometry
                            .normalize(state.ctm.point(values[4], values[5])),
                        page.number,
                        offset,
                        "curve endpoint",
                    )?;
                    path.push(PathCommand::CurveTo { c1, c2, to });
                    current = Some(to);
                }
                "v" => {
                    let values = numbers(operation, 4, page.number, offset)?;
                    let c1 = current.ok_or_else(|| {
                        import_error(Some(page.number), Some(offset), "v has no current point")
                    })?;
                    let c2 = checked_point(
                        page.geometry
                            .normalize(state.ctm.point(values[0], values[1])),
                        page.number,
                        offset,
                        "curve control point",
                    )?;
                    let to = checked_point(
                        page.geometry
                            .normalize(state.ctm.point(values[2], values[3])),
                        page.number,
                        offset,
                        "curve endpoint",
                    )?;
                    path.push(PathCommand::CurveTo { c1, c2, to });
                    current = Some(to);
                }
                "y" => {
                    require_current_point(current, page.number, offset, "y")?;
                    let values = numbers(operation, 4, page.number, offset)?;
                    let c1 = checked_point(
                        page.geometry
                            .normalize(state.ctm.point(values[0], values[1])),
                        page.number,
                        offset,
                        "curve control point",
                    )?;
                    let to = checked_point(
                        page.geometry
                            .normalize(state.ctm.point(values[2], values[3])),
                        page.number,
                        offset,
                        "curve endpoint",
                    )?;
                    path.push(PathCommand::CurveTo { c1, c2: to, to });
                    current = Some(to);
                }
                "h" => {
                    require_current_point(current, page.number, offset, "h")?;
                    let start = subpath.ok_or_else(|| {
                        import_error(Some(page.number), Some(offset), "h has no current subpath")
                    })?;
                    path.push(PathCommand::Close);
                    current = Some(start);
                }
                "re" => {
                    let values = numbers(operation, 4, page.number, offset)?;
                    let right = checked_sum(
                        values[0],
                        values[2],
                        page.number,
                        offset,
                        "rectangle x extent",
                    )?;
                    let top = checked_sum(
                        values[1],
                        values[3],
                        page.number,
                        offset,
                        "rectangle y extent",
                    )?;
                    let points = [
                        (values[0], values[1]),
                        (right, values[1]),
                        (right, top),
                        (values[0], top),
                    ];
                    for (index, (x, y)) in points.into_iter().enumerate() {
                        let p = checked_point(
                            page.geometry.normalize(state.ctm.point(x, y)),
                            page.number,
                            offset,
                            "rectangle corner",
                        )?;
                        path.push(if index == 0 {
                            PathCommand::MoveTo(p)
                        } else {
                            PathCommand::LineTo(p)
                        });
                    }
                    path.push(PathCommand::Close);
                    current = path.iter().rev().find_map(|command| match command {
                        PathCommand::MoveTo(point) => Some(*point),
                        _ => None,
                    });
                    subpath = current;
                }
                "S" | "s" | "f" | "F" | "B" | "b" => {
                    if matches!(operation.operator.as_str(), "s" | "b") {
                        require_current_point(current, page.number, offset, &operation.operator)?;
                        path.push(PathCommand::Close);
                    }
                    let fill = (state.graphics_supported
                        && state.fill_supported
                        && matches!(operation.operator.as_str(), "f" | "F" | "B" | "b"))
                    .then_some(Paint::Solid(state.fill));
                    let stroke = if state.graphics_supported
                        && state.stroke_supported
                        && state.dash_supported
                        && matches!(operation.operator.as_str(), "S" | "s" | "B" | "b")
                    {
                        self.transformed_stroke(page.number, offset, &state)?
                    } else {
                        None
                    };
                    self.paint_path(page, &mut path, fill, stroke)?;
                    current = None;
                    subpath = None;
                }
                "f*" | "B*" | "b*" => {
                    self.diagnostic(page.number, offset, "even-odd path painting is unsupported")?;
                    path.clear();
                    current = None;
                    subpath = None;
                }
                "n" => {
                    path.clear();
                    current = None;
                    subpath = None;
                }
                "w" => {
                    state.line_width = finite_nonnegative(
                        number_at(operation, 0, page.number, offset)?,
                        page.number,
                        offset,
                        "line width",
                    )?
                }
                "J" => {
                    state.line_cap = match integer_at(operation, 0, page.number, offset)? {
                        0 => LineCap::Butt,
                        1 => LineCap::Round,
                        2 => LineCap::Square,
                        value => {
                            return Err(import_error(
                                Some(page.number),
                                Some(offset),
                                format!("invalid line cap {value}"),
                            ));
                        }
                    }
                }
                "j" => {
                    state.line_join = match integer_at(operation, 0, page.number, offset)? {
                        0 => LineJoin::Miter,
                        1 => LineJoin::Round,
                        2 => LineJoin::Bevel,
                        value => {
                            return Err(import_error(
                                Some(page.number),
                                Some(offset),
                                format!("invalid line join {value}"),
                            ));
                        }
                    }
                }
                "d" => {
                    let dash = parse_dash(operation, page.number, offset)?;
                    if dash
                        .as_ref()
                        .is_some_and(|dash| dash.pattern.contains(&0.0))
                    {
                        self.diagnostic(
                            page.number,
                            offset,
                            "zero-length dash members are unsupported; dependent strokes are isolated until dash state reset",
                        )?;
                        state.dash = None;
                        state.dash_supported = false;
                    } else if let Some(dash) = dash {
                        if let Some(pattern) = normalized_dash(&dash.pattern, dash.phase) {
                            state.dash = Some(DashState {
                                pattern,
                                phase: 0.0,
                            });
                            state.dash_supported = true;
                        } else {
                            self.diagnostic(
                                page.number,
                                offset,
                                "dash phase requires a zero DrawingML dash stop; dependent strokes are isolated until dash state reset",
                            )?;
                            state.dash = None;
                            state.dash_supported = false;
                        }
                    } else {
                        state.dash = None;
                        state.dash_supported = true;
                    }
                }
                "g" => {
                    state.fill = gray(
                        number_at(operation, 0, page.number, offset)?,
                        page.number,
                        offset,
                    )?;
                    state.fill_supported = true;
                }
                "G" => {
                    state.stroke = gray(
                        number_at(operation, 0, page.number, offset)?,
                        page.number,
                        offset,
                    )?;
                    state.stroke_supported = true;
                }
                "rg" => {
                    state.fill = rgb(operation, page.number, offset)?;
                    state.fill_supported = true;
                }
                "RG" => {
                    state.stroke = rgb(operation, page.number, offset)?;
                    state.stroke_supported = true;
                }
                "BT" => {
                    if state.in_text {
                        return Err(import_error(
                            Some(page.number),
                            Some(offset),
                            "nested BT operator",
                        ));
                    }
                    state.in_text = true;
                    state.text_matrix = Matrix::IDENTITY;
                    state.line_matrix = Matrix::IDENTITY;
                    state.text_position_supported = true;
                }
                "ET" => {
                    if !state.in_text {
                        return Err(import_error(
                            Some(page.number),
                            Some(offset),
                            "ET without BT",
                        ));
                    }
                    state.in_text = false;
                }
                "Tf" => {
                    require_text(&state, page.number, offset)?;
                    let name = name_at(operation, 0, page.number, offset)?.to_vec();
                    state.font_resource = Some(name.clone());
                    state.font_size = finite_positive(
                        number_at(operation, 1, page.number, offset)?,
                        page.number,
                        offset,
                        "font size",
                    )?;
                    state.text_metrics_supported = fonts
                        .get(&name)
                        .is_none_or(|font| font.composite_widths_supported);
                    if !state.text_metrics_supported {
                        self.diagnostic(
                            page.number,
                            offset,
                            format!(
                                "composite font /{} widths are unsupported; dependent text is isolated",
                                String::from_utf8_lossy(&name)
                            ),
                        )?;
                    }
                }
                "Tm" => {
                    require_text(&state, page.number, offset)?;
                    let v = numbers(operation, 6, page.number, offset)?;
                    state.text_matrix = Matrix {
                        a: v[0],
                        b: v[1],
                        c: v[2],
                        d: v[3],
                        e: v[4],
                        f: v[5],
                    };
                    state.line_matrix = state.text_matrix;
                    state.text_position_supported = true;
                }
                "Td" | "TD" => {
                    require_text(&state, page.number, offset)?;
                    let v = numbers(operation, 2, page.number, offset)?;
                    if operation.operator == "TD" {
                        state.leading = -v[1];
                    }
                    state.line_matrix = checked_matrix(
                        state.line_matrix.translated(v[0], v[1]),
                        page.number,
                        offset,
                        "text line translation",
                    )?;
                    state.text_matrix = state.line_matrix;
                    state.text_position_supported = true;
                }
                "T*" => {
                    require_text(&state, page.number, offset)?;
                    state.line_matrix = checked_matrix(
                        state.line_matrix.translated(0.0, -state.leading),
                        page.number,
                        offset,
                        "text leading translation",
                    )?;
                    state.text_matrix = state.line_matrix;
                    state.text_position_supported = true;
                }
                "Tc" => {
                    require_text(&state, page.number, offset)?;
                    state.character_spacing = number_at(operation, 0, page.number, offset)?;
                }
                "Tw" => {
                    require_text(&state, page.number, offset)?;
                    state.word_spacing = number_at(operation, 0, page.number, offset)?;
                }
                "Tz" => {
                    require_text(&state, page.number, offset)?;
                    state.horizontal_scale = number_at(operation, 0, page.number, offset)? / 100.0;
                    if !state.horizontal_scale.is_finite() || state.horizontal_scale <= 0.0 {
                        return Err(import_error(
                            Some(page.number),
                            Some(offset),
                            "horizontal text scale must be finite and positive",
                        ));
                    }
                }
                "TL" => {
                    require_text(&state, page.number, offset)?;
                    state.leading = number_at(operation, 0, page.number, offset)?;
                }
                "Ts" => {
                    require_text(&state, page.number, offset)?;
                    state.rise = number_at(operation, 0, page.number, offset)?;
                }
                "Tr" => {
                    require_text(&state, page.number, offset)?;
                    let mode = integer_at(operation, 0, page.number, offset)?;
                    if !(0..=7).contains(&mode) {
                        return Err(import_error(
                            Some(page.number),
                            Some(offset),
                            format!("invalid text rendering mode {mode}"),
                        ));
                    }
                    state.text_visible = mode == 0;
                    if (4..=7).contains(&mode) {
                        state.graphics_supported = false;
                    }
                    if mode != 0 && mode != 3 {
                        self.diagnostic(
                            page.number,
                            offset,
                            format!("text rendering mode {mode} is unsupported"),
                        )?;
                    }
                }
                "Tj" => {
                    let text = string_bytes_at(operation, 0, page.number, offset)?;
                    self.show_text(page, offset, &mut state, fonts, &text)?;
                }
                "TJ" => {
                    let array = operation
                        .operands
                        .first()
                        .and_then(|value| value.as_array().ok())
                        .ok_or_else(|| {
                            import_error(Some(page.number), Some(offset), "TJ requires an array")
                        })?;
                    for item in array {
                        match item {
                            Object::String(text, _) => {
                                self.show_text(page, offset, &mut state, fonts, text)?;
                            }
                            Object::Integer(value) => {
                                let adjustment = -(*value as f64) / 1000.0
                                    * state.font_size
                                    * state.horizontal_scale;
                                advance_text_matrix(&mut state, adjustment, page.number, offset)?;
                            }
                            Object::Real(value) => {
                                let adjustment = -f64::from(*value) / 1000.0
                                    * state.font_size
                                    * state.horizontal_scale;
                                advance_text_matrix(&mut state, adjustment, page.number, offset)?;
                            }
                            _ => {
                                return Err(import_error(
                                    Some(page.number),
                                    Some(offset),
                                    "TJ array contains a non-string, non-number operand",
                                ));
                            }
                        }
                    }
                }
                "'" | "\"" => {
                    require_text(&state, page.number, offset)?;
                    if operation.operator == "\"" {
                        state.word_spacing = number_at(operation, 0, page.number, offset)?;
                        state.character_spacing = number_at(operation, 1, page.number, offset)?;
                    }
                    state.line_matrix = checked_matrix(
                        state.line_matrix.translated(0.0, -state.leading),
                        page.number,
                        offset,
                        "text leading translation",
                    )?;
                    state.text_matrix = state.line_matrix;
                    state.text_position_supported = true;
                    let index = if operation.operator == "\"" { 2 } else { 0 };
                    let text = string_bytes_at(operation, index, page.number, offset)?;
                    self.show_text(page, offset, &mut state, fonts, &text)?;
                }
                "Do" => {
                    let name = name_at(operation, 0, page.number, offset)?;
                    if !state.graphics_supported {
                        self.diagnostic(
                            page.number,
                            offset,
                            "XObject omitted while unsupported graphics state is active",
                        )?;
                    } else if let Some(id) = xobjects.get(name) {
                        self.show_xobject(page, offset, &state, *id)?;
                    } else {
                        self.diagnostic(
                            page.number,
                            offset,
                            format!("missing XObject /{}", String::from_utf8_lossy(name)),
                        )?;
                    }
                }
                "W" | "W*" => {
                    self.diagnostic(
                        page.number,
                        offset,
                        "content clipping path is unsupported and dependent content is isolated",
                    )?;
                    state.graphics_supported = false;
                }
                "CS" | "SC" | "SCN" => {
                    self.diagnostic(
                        page.number,
                        offset,
                        format!("colour operator {} is unsupported", operation.operator),
                    )?;
                    state.stroke_supported = false;
                }
                "cs" | "sc" | "scn" => {
                    self.diagnostic(
                        page.number,
                        offset,
                        format!("colour operator {} is unsupported", operation.operator),
                    )?;
                    state.fill_supported = false;
                }
                "gs" => {
                    self.diagnostic(
                        page.number,
                        offset,
                        "extended graphics state is unsupported and dependent content is isolated",
                    )?;
                    state.graphics_supported = false;
                }
                "sh" => self.diagnostic(page.number, offset, "shading is unsupported")?,
                "BI" | "ID" | "EI" => {
                    self.diagnostic(page.number, offset, "inline image is unsupported")?
                }
                other => self.diagnostic(
                    page.number,
                    offset,
                    format!("unsupported PDF operator {other}"),
                )?,
            }
        }
        if !stack.is_empty() {
            return Err(import_error(
                Some(page.number),
                None,
                "unbalanced q operator",
            ));
        }
        if state.in_text {
            return Err(import_error(
                Some(page.number),
                None,
                "unbalanced BT operator",
            ));
        }
        Ok(())
    }

    fn transformed_stroke(
        &mut self,
        page: u32,
        offset: usize,
        state: &GraphicsState,
    ) -> Result<Option<Stroke>> {
        let x_scale = state.ctm.a.hypot(state.ctm.b);
        let y_scale = state.ctm.c.hypot(state.ctm.d);
        let dot = state.ctm.a * state.ctm.c + state.ctm.b * state.ctm.d;
        checked_finite(x_scale, page, offset, "stroke x scale")?;
        checked_finite(y_scale, page, offset, "stroke y scale")?;
        checked_finite(dot, page, offset, "stroke axis product")?;
        let tolerance = x_scale.max(y_scale).max(1.0) * 1.0e-9;
        if x_scale <= f64::EPSILON
            || y_scale <= f64::EPSILON
            || dot.abs() > tolerance
            || (x_scale - y_scale).abs() > tolerance
        {
            self.diagnostic(
                page,
                offset,
                "stroked path omitted because a nonuniform or sheared CTM cannot preserve its outline",
            )?;
            return Ok(None);
        }
        let width = checked_product(state.line_width, x_scale, page, offset, "stroke width")?;
        let dash = if let Some(dash) = &state.dash {
            let mut transformed = Vec::new();
            for value in &dash.pattern {
                transformed.push(checked_product(
                    *value,
                    x_scale,
                    page,
                    offset,
                    "stroke dash length",
                )?);
            }
            if transformed
                .iter()
                .any(|value| drawingml_dash_units(*value, width).is_none())
            {
                self.diagnostic(
                    page,
                    offset,
                    "dash member is too small for a positive DrawingML dash stop; affected stroke is omitted",
                )?;
                return Ok(None);
            }
            Some(transformed)
        } else {
            None
        };
        Ok(Some(Stroke {
            paint: Paint::Solid(state.stroke),
            width,
            cap: state.line_cap,
            join: state.line_join,
            dash,
        }))
    }

    fn paint_path(
        &mut self,
        page: &mut NormalizedPage,
        commands: &mut Vec<PathCommand>,
        fill: Option<Paint>,
        stroke: Option<Stroke>,
    ) -> Result<()> {
        if commands.is_empty() {
            return Ok(());
        }
        if fill.is_none() && stroke.is_none() {
            commands.clear();
            return Ok(());
        }
        self.ensure_shape_room(page, 1)?;
        let element = PathElement {
            path: LayoutPath {
                commands: std::mem::take(commands),
                fill_rule: FillRule::NonZero,
            },
            fill,
            stroke,
        };
        let bounds = element
            .path
            .bounds()
            .ok_or_else(|| import_error(Some(page.number), None, "painted path has no bounds"))?;
        checked_rect(bounds, page.number, 0, "painted path bounds")?;
        page.layout.push(PositionedElement::Path(element.clone()));
        page.editable.push(EditableElement::Path(element));
        Ok(())
    }

    fn show_text(
        &mut self,
        page: &mut NormalizedPage,
        offset: usize,
        state: &mut GraphicsState,
        fonts: &BTreeMap<Vec<u8>, PageFont>,
        text_bytes: &[u8],
    ) -> Result<()> {
        require_text(state, page.number, offset)?;
        if !state.text_metrics_supported {
            state.text_position_supported = false;
            return Ok(());
        }
        if !state.text_position_supported {
            return Ok(());
        }
        let font = state
            .font_resource
            .as_ref()
            .and_then(|name| fonts.get(name));
        let text = if let Some(font) = font {
            if !font.encoding_supported {
                self.diagnostic(
                    page.number,
                    offset,
                    format!(
                        "font `{}` uses unsupported encoding `{}`; bytes use deterministic direct decoding",
                        font.display_family, font.encoding
                    ),
                )?;
                decode_pdf_string(text_bytes)
            } else {
                font.decoded
                    .get(text_bytes)
                    .ok_or_else(|| {
                        import_error(
                            Some(page.number),
                            Some(offset),
                            format!("font encoding `{}` was not cached", font.encoding),
                        )
                    })?
                    .clone()
            }
        } else {
            self.diagnostic(
                page.number,
                offset,
                "missing font resource; bytes use deterministic direct decoding",
            )?;
            decode_pdf_string(text_bytes)
        };
        if text.is_empty() {
            return Ok(());
        }
        let requested = font.map(|font| font.shaping_family.clone());
        let family = requested.clone().unwrap_or_else(|| "Carlito".to_owned());
        let (font_id, mut substituted) = match self.fonts.resolve_font(Some(&family), false, false)
        {
            Ok(id) => (id, false),
            Err(_) => (
                self.fonts
                    .resolve_font(Some("Carlito"), false, false)
                    .map_err(|error| {
                        import_error(
                            Some(page.number),
                            Some(offset),
                            format!("font resolution failed: {error}"),
                        )
                    })?,
                true,
            ),
        };
        let actual_family = self
            .fonts
            .font_data(font_id)
            .map_err(|error| {
                import_error(
                    Some(page.number),
                    Some(offset),
                    format!("resolved font data failed: {error}"),
                )
            })?
            .family;
        substituted |= !self.embedded_font_aliases.contains(&family);
        if requested.is_none() || substituted {
            self.diagnostic(
                page.number,
                offset,
                format!("font `{family}` substituted with deterministic `{actual_family}`"),
            )?;
        }
        let combined = checked_matrix(
            state.text_matrix.then(state.ctm),
            page.number,
            offset,
            "combined text transform",
        )?;
        let origin = checked_point(
            page.geometry.normalize(combined.point(0.0, state.rise)),
            page.number,
            offset,
            "text origin",
        )?;
        let x_axis = checked_point(
            page.geometry.normalize(combined.point(1.0, state.rise)),
            page.number,
            offset,
            "text x axis",
        )?;
        let y_axis = checked_point(
            page.geometry
                .normalize(combined.point(0.0, state.rise + 1.0)),
            page.number,
            offset,
            "text y axis",
        )?;
        let x_vector = Point {
            x: (x_axis.x - origin.x) * state.horizontal_scale,
            y: (x_axis.y - origin.y) * state.horizontal_scale,
        };
        let y_vector = Point {
            x: origin.x - y_axis.x,
            y: origin.y - y_axis.y,
        };
        let x_scale = point_distance(Point { x: 0.0, y: 0.0 }, x_vector);
        let y_scale = point_distance(Point { x: 0.0, y: 0.0 }, y_vector);
        checked_finite(x_scale, page.number, offset, "text x scale")?;
        checked_finite(y_scale, page.number, offset, "text y scale")?;
        if x_scale <= f64::EPSILON || y_scale <= f64::EPSILON {
            return Err(import_error(
                Some(page.number),
                Some(offset),
                "text transform is singular",
            ));
        }
        let mut shaped = self
            .fonts
            .shape_text(font_id, &text, state.font_size)
            .map_err(|error| {
                import_error(
                    Some(page.number),
                    Some(offset),
                    format!("text shaping failed: {error}"),
                )
            })?;
        let characters = text.chars().collect::<Vec<_>>();
        let source_advances = font
            .and_then(|font| font.widths.as_ref())
            .map(|widths| {
                text_bytes
                    .iter()
                    .map(|byte| {
                        let index = i64::from(*byte) - widths.first_char;
                        let width = usize::try_from(index)
                            .ok()
                            .and_then(|index| widths.widths.get(index))
                            .copied()
                            .unwrap_or(widths.missing_width);
                        let mut advance =
                            width / 1000.0 * state.font_size + state.character_spacing;
                        if *byte == b' ' {
                            advance += state.word_spacing;
                        }
                        checked_finite(advance, page.number, offset, "source font advance")
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?;
        let unscaled_advance = if let Some(source_advances) = source_advances {
            if source_advances.len() == shaped.advances.len() {
                shaped.advances.clone_from(&source_advances);
            }
            source_advances.iter().sum()
        } else {
            for (index, advance) in shaped.advances.iter_mut().enumerate() {
                *advance += state.character_spacing;
                if characters.get(index) == Some(&' ') {
                    *advance += state.word_spacing;
                }
            }
            let word_count = text.chars().filter(|character| *character == ' ').count() as f64;
            shaped.width
                + state.character_spacing * characters.len() as f64
                + state.word_spacing * word_count
        };
        let text_advance = checked_product(
            unscaled_advance,
            state.horizontal_scale,
            page.number,
            offset,
            "text advance",
        )?;
        let run = GlyphRun {
            origin: Point { x: 0.0, y: 0.0 },
            font_id,
            font_size: state.font_size,
            glyph_ids: shaped.glyph_ids,
            advances: shaped.advances,
            text: text.clone(),
            source: None,
            color: state.fill,
            bold: false,
            italic: false,
            field_kind: None,
            field_source: None,
            note: None,
            note_reference_source: None,
            tab_aligned: None,
        };
        if state.graphics_supported && state.fill_supported && state.text_visible {
            self.ensure_shape_room(page, 1)?;
            let transform = Transform {
                a: x_vector.x,
                b: x_vector.y,
                c: y_vector.x,
                d: y_vector.y,
                e: origin.x,
                f: origin.y,
            };
            page.layout.push(PositionedElement::Group(GroupElement {
                transform,
                clip: None,
                opacity: 1.0,
                effects: Vec::new(),
                children: vec![PositionedElement::Text(run)],
            }));
            // The box top sits where a single-spaced first line puts the
            // baseline this far down, in PowerPoint and in the renderer: 1.2
            // em less the descent, and no higher than three quarters of the
            // line or the font's own ascent share.
            let metrics = self
                .fonts
                .metrics(font_id, state.font_size)
                .map_err(|error| {
                    import_error(
                        Some(page.number),
                        Some(offset),
                        format!("text metrics failed: {error}"),
                    )
                })?;
            let line = 1.2 * state.font_size;
            let share = (metrics.ascent / (metrics.ascent + metrics.descent)).min(0.75);
            let rect = Rect {
                x: 0.0,
                y: -(line - metrics.descent).max(line * share),
                width: unscaled_advance.abs().max(state.font_size * 0.25),
                height: state.font_size * 1.4,
            };
            checked_rect(rect, page.number, offset, "editable text local rectangle")?;
            page.editable.push(EditableElement::Text {
                rect,
                transform,
                text,
                family: if substituted {
                    actual_family
                } else {
                    font.map_or(family, |font| font.display_family.clone())
                },
                size: state.font_size,
                color: state.fill,
            });
        }
        advance_text_matrix(state, text_advance, page.number, offset)?;
        Ok(())
    }

    fn show_xobject(
        &mut self,
        page: &mut NormalizedPage,
        offset: usize,
        state: &GraphicsState,
        id: ObjectId,
    ) -> Result<()> {
        let stream = self
            .document
            .get_object(id)
            .and_then(Object::as_stream)
            .map_err(|error| {
                import_error(
                    Some(page.number),
                    Some(offset),
                    format!("XObject failed: {error}"),
                )
            })?
            .clone();
        let subtype = stream
            .dict
            .get(b"Subtype")
            .and_then(Object::as_name)
            .unwrap_or_default();
        if subtype != b"Image" {
            self.diagnostic(page.number, offset, "form XObject recursion is unsupported")?;
            return Ok(());
        }
        let width = positive_i64(&stream.dict, b"Width", page.number, offset)?;
        let height = positive_i64(&stream.dict, b"Height", page.number, offset)?;
        let width = u32::try_from(width).map_err(|_| {
            import_error(
                Some(page.number),
                Some(offset),
                "image Width exceeds the supported integer range",
            )
        })?;
        let height = u32::try_from(height).map_err(|_| {
            import_error(
                Some(page.number),
                Some(offset),
                "image Height exceeds the supported integer range",
            )
        })?;
        let corners = image_corners(page.geometry, state.ctm, page.number, offset)?;
        let bounds = points_bounds(&corners);
        checked_rect(bounds, page.number, offset, "image bounds")?;
        if bounds.width <= f64::EPSILON || bounds.height <= f64::EPSILON {
            return Err(import_error(
                Some(page.number),
                Some(offset),
                "image transform has empty page bounds",
            ));
        }
        let transform = Transform {
            a: corners[1].x - corners[0].x,
            b: corners[1].y - corners[0].y,
            c: corners[2].x - corners[0].x,
            d: corners[2].y - corners[0].y,
            e: corners[0].x,
            f: corners[0].y,
        };
        let determinant = transform.a * transform.d - transform.b * transform.c;
        if !determinant.is_finite() || determinant.abs() <= f64::EPSILON {
            return Err(import_error(
                Some(page.number),
                Some(offset),
                "image transform is singular",
            ));
        }
        let filters = filter_names(&stream.dict)
            .map_err(|message| import_error(Some(page.number), Some(offset), message))?;
        if let Some(message) = unsupported_image_semantics(&stream.dict, &filters)
            .map_err(|message| import_error(Some(page.number), Some(offset), message))?
        {
            self.diagnostic(page.number, offset, message)?;
            return Ok(());
        }
        let intrinsic = if filters.as_slice() == ["DCTDecode"] {
            let intrinsic = jpeg_dimensions(&stream.content)
                .map_err(|message| import_error(Some(page.number), Some(offset), message))?;
            if intrinsic != (width, height) {
                return Err(import_error(
                    Some(page.number),
                    Some(offset),
                    format!(
                        "JPEG intrinsic dimensions {}x{} do not match declared image dimensions {width}x{height}",
                        intrinsic.0, intrinsic.1
                    ),
                ));
            }
            intrinsic
        } else {
            (width, height)
        };
        let pixels = u64::from(intrinsic.0)
            .checked_mul(u64::from(intrinsic.1))
            .ok_or_else(|| {
                import_error(
                    Some(page.number),
                    Some(offset),
                    "image pixel count overflow",
                )
            })?;
        page.image_pixels = page.image_pixels.checked_add(pixels).ok_or_else(|| {
            import_error(
                Some(page.number),
                Some(offset),
                "aggregate page image pixel count overflow",
            )
        })?;
        if page.image_pixels > self.limits.max_pixels_per_page {
            return Err(import_error(
                Some(page.number),
                Some(offset),
                format!(
                    "aggregate page image pixel count {} exceeds limit {}",
                    page.image_pixels, self.limits.max_pixels_per_page
                ),
            ));
        }
        let (data, filename, content_type) = if filters.as_slice() == ["DCTDecode"] {
            (
                stream.content.clone(),
                "image.jpg".to_owned(),
                "image/jpeg".to_owned(),
            )
        } else if filters.is_empty() || filters.as_slice() == ["FlateDecode"] {
            let remaining = self.remaining_decompressed(Some(page.number), Some(offset))?;
            let expected = decoded_image_len(&stream.dict, width, height)
                .map_err(|message| import_error(Some(page.number), Some(offset), message))?;
            if expected > remaining {
                return Err(import_error(
                    Some(page.number),
                    Some(offset),
                    format!(
                        "aggregate decompressed bytes would exceed limit {} while decoding image",
                        self.limits.max_decompressed_bytes
                    ),
                ));
            }
            match decode_png(&stream, width, height, remaining) {
                Ok((data, decoded)) => {
                    self.charge_decompressed(decoded, Some(page.number), Some(offset), "image")?;
                    (data, "image.png".to_owned(), "image/png".to_owned())
                }
                Err(message) => {
                    self.diagnostic(page.number, offset, message)?;
                    return Ok(());
                }
            }
        } else {
            self.diagnostic(
                page.number,
                offset,
                format!("image filters {} are unsupported", filters.join(",")),
            )?;
            return Ok(());
        };
        self.ensure_shape_room(page, 1)?;
        let image = PositionedElement::Image {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            },
            media_id: MediaId::from_bytes(&data),
            data: data.clone(),
            content_type,
        };
        page.layout.push(PositionedElement::Group(GroupElement {
            transform,
            clip: None,
            opacity: 1.0,
            effects: Vec::new(),
            children: vec![image],
        }));
        page.editable.push(EditableElement::Image {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            },
            transform,
            data,
            filename,
        });
        Ok(())
    }

    fn parse_annotations(&mut self, page: &mut NormalizedPage, id: ObjectId) -> Result<()> {
        let mut resolver = ResourceReferenceResolver::new(self.limits.max_objects);
        let ids = strict_annotation_ids(
            &self.document,
            id,
            page.number,
            page.retained_elements,
            self.limits,
            &mut resolver,
        )?;
        self.ensure_shape_room(page, ids.len())?;
        let mut seen = HashSet::new();
        for (index, id) in ids.into_iter().enumerate() {
            match strict_annotation(&self.document, page, index, id, &mut resolver, &mut seen)? {
                AnnotationDecision::Skip => {}
                AnnotationDecision::Diagnostic(message) => {
                    self.annotation_diagnostic(page.number, index, message)?;
                }
                AnnotationDecision::Link { rect, uri } => {
                    page.layout.push(PositionedElement::LinkAnnotation {
                        rect,
                        url: uri.clone(),
                    });
                    page.links.push((rect, uri));
                }
            }
        }
        Ok(())
    }

    fn ensure_shape_room(&self, page: &mut NormalizedPage, additional: usize) -> Result<()> {
        let retained = page.retained_elements.saturating_add(additional);
        if retained > self.limits.max_shapes_per_page {
            Err(import_error(
                Some(page.number),
                None,
                format!(
                    "page shape count exceeds limit {}",
                    self.limits.max_shapes_per_page
                ),
            ))
        } else {
            page.retained_elements = retained;
            Ok(())
        }
    }

    fn annotation_diagnostic(
        &mut self,
        page: u32,
        index: usize,
        message: impl Into<String>,
    ) -> Result<()> {
        if self.diagnostics.len() >= self.limits.max_diagnostics {
            return Err(import_error(
                Some(page),
                None,
                format!(
                    "diagnostic count exceeds limit {}",
                    self.limits.max_diagnostics
                ),
            ));
        }
        self.diagnostics.push(PdfImportDiagnostic {
            path: format!("pdf/page[{page}]/annotation[{index}]"),
            message: message.into(),
        });
        Ok(())
    }

    fn project(&mut self, pages: &[NormalizedPage]) -> Result<Presentation> {
        let mut presentation = Presentation::from_bytes(DEFAULT_TEMPLATE)?;
        while !presentation.slides.is_empty() {
            presentation.remove_slide(0)?;
        }
        presentation.set_slide_size(
            points_to_emu(pages[0].geometry.width)?,
            points_to_emu(pages[0].geometry.height)?,
        )?;
        let font_data = self.fonts.all_font_data();
        for (index, page) in pages.iter().enumerate() {
            presentation.add_slide(0)?;
            Arc::make_mut(&mut presentation.slides[index])
                .slide
                .common_slide_data
                .shape_tree
                .children
                .clear();
            match self.mode {
                PdfImportMode::PreservedGraphic { dpi } => {
                    let pixels = raster_pixels(page.geometry.width, page.geometry.height, dpi)?;
                    if pixels > self.limits.max_pixels_per_page {
                        return Err(import_error(
                            Some(page.number),
                            None,
                            format!(
                                "raster pixel count {pixels} exceeds limit {}",
                                self.limits.max_pixels_per_page
                            ),
                        ));
                    }
                    let mut frame = PageFrame::new(
                        index + 1,
                        page.geometry.width,
                        page.geometry.height,
                        page.layout.clone(),
                    );
                    frame.background = Some(Paint::Solid(Color::WHITE));
                    let layout = LayoutResult::new(
                        vec![Arc::new(frame)],
                        font_data.clone(),
                        None,
                        Vec::new(),
                    );
                    let png =
                        oxoffice_pdf::render_page_to_png(&layout, 0, dpi).ok_or_else(|| {
                            import_error(Some(page.number), None, "shared PDF rasterizer failed")
                        })?;
                    presentation.add_picture(
                        index,
                        &png,
                        "page.png",
                        Emu(0),
                        Emu(0),
                        Some(points_to_emu(page.geometry.width)?),
                        Some(points_to_emu(page.geometry.height)?),
                    )?;
                }
                PdfImportMode::Editable => self.project_editable(&mut presentation, index, page)?,
            }
        }
        let bytes = presentation.to_bytes()?;
        let reopened = Presentation::from_bytes(&bytes)?;
        let issues = reopened.validate();
        if !issues.is_empty() {
            return Err(import_error(
                None,
                None,
                format!(
                    "generated presentation failed validation with {} issue(s)",
                    issues.len()
                ),
            ));
        }
        Ok(reopened)
    }

    fn project_editable(
        &self,
        presentation: &mut Presentation,
        slide_index: usize,
        page: &NormalizedPage,
    ) -> Result<()> {
        for element in &page.editable {
            match element {
                EditableElement::Text {
                    rect,
                    transform,
                    text,
                    family,
                    size,
                    color,
                } => {
                    let affine = editable_affine(*rect, *transform)?;
                    let slide_count = presentation.slides.len();
                    let mut slide =
                        presentation
                            .slide_mut(slide_index)
                            .ok_or(Error::UnknownSlideIndex {
                                index: slide_index,
                                slide_count,
                            })?;
                    let mut shape = slide.add_textbox(
                        rect_emu(*rect)?.0,
                        rect_emu(*rect)?.1,
                        rect_emu(*rect)?.2,
                        rect_emu(*rect)?.3,
                    )?;
                    shape.set_text(text)?;
                    shape.set_rotation(affine.child_rotation)?;
                    if let Some(frame) = shape.text_frame() {
                        frame.body.body_properties.left_inset = Some(Coordinate32Value::Emu(0));
                        frame.body.body_properties.top_inset = Some(Coordinate32Value::Emu(0));
                        frame.body.body_properties.right_inset = Some(Coordinate32Value::Emu(0));
                        frame.body.body_properties.bottom_inset = Some(Coordinate32Value::Emu(0));
                    }
                    if let Some(frame) = shape.text_frame()
                        && let Some(mut paragraph) = frame.into_paragraph_mut(0)
                        && let Some(mut run) = paragraph.run_mut(0)
                    {
                        let mut properties = CT_TextCharacterProperties::default();
                        properties.font_size = Some((size * 100.0).trunc() as i32);
                        properties.latin =
                            Some(TextFont::new(family.clone()).map_err(|error| {
                                import_error(
                                    None,
                                    None,
                                    format!("text font construction failed: {error}"),
                                )
                            })?);
                        properties.fill = Some(solid_fill(*color)?);
                        run.set_properties(properties);
                    }
                    let _ = shape;
                    wrap_last_shape_in_affine_group(
                        presentation,
                        slide_index,
                        affine.group_transform,
                    )?;
                }
                EditableElement::Path(path) => append_custom_path(presentation, slide_index, path)?,
                EditableElement::Image {
                    rect,
                    transform,
                    data,
                    filename,
                } => {
                    let affine = editable_affine(*rect, *transform)?;
                    let (left, top, width, height) = rect_emu(*rect)?;
                    presentation.add_picture(
                        slide_index,
                        data,
                        filename,
                        left,
                        top,
                        Some(width),
                        Some(height),
                    )?;
                    let shape_count = presentation
                        .slide(slide_index)
                        .map(|slide| slide.shapes().len())
                        .ok_or(Error::UnknownSlideIndex {
                            index: slide_index,
                            slide_count: presentation.slides.len(),
                        })?;
                    let mut shape = presentation
                        .slide_mut(slide_index)
                        .and_then(|slide| slide.into_shape_mut(shape_count - 1))
                        .ok_or_else(|| {
                            import_error(None, None, "inserted PDF image was not retained")
                        })?;
                    shape.set_rotation(affine.child_rotation)?;
                    let _ = shape;
                    wrap_last_shape_in_affine_group(
                        presentation,
                        slide_index,
                        affine.group_transform,
                    )?;
                }
            }
        }
        for (rect, uri) in &page.links {
            append_link_overlay(presentation, slide_index, *rect, uri)?;
        }
        Ok(())
    }
}

struct EditableAffine {
    group_transform: CT_Transform2D,
    child_rotation: Angle,
}

fn editable_affine(rect: Rect, transform: Transform) -> Result<EditableAffine> {
    let p = transform.a * transform.a + transform.b * transform.b;
    let q = transform.a * transform.c + transform.b * transform.d;
    let r = transform.c * transform.c + transform.d * transform.d;
    let discriminant = (p - r).hypot(2.0 * q);
    let first = ((p + r + discriminant) / 2.0).sqrt();
    let second = ((p + r - discriminant) / 2.0).max(0.0).sqrt();
    if !first.is_finite() || !second.is_finite() || first <= f64::EPSILON || second <= f64::EPSILON
    {
        return Err(import_error(
            None,
            None,
            "editable affine transform is singular",
        ));
    }

    let theta = 0.5 * (2.0 * q).atan2(p - r);
    let (sin, cos) = theta.sin_cos();
    let u1_x = (transform.a * cos + transform.c * sin) / first;
    let u1_y = (transform.b * cos + transform.d * sin) / first;
    let u2_x = (-transform.a * sin + transform.c * cos) / second;
    let u2_y = (-transform.b * sin + transform.d * cos) / second;
    let determinant = u1_x * u2_y - u1_y * u2_x;
    let (rotation, flip_vertical) = if determinant < 0.0 {
        ((-u1_y).atan2(u1_x).to_degrees(), true)
    } else {
        (u1_y.atan2(u1_x).to_degrees(), false)
    };

    let local_center = Point {
        x: rect.x + rect.width / 2.0,
        y: rect.y + rect.height / 2.0,
    };
    let target_center = transform.apply(local_center);
    let extent_width = first * rect.width;
    let extent_height = second * rect.height;
    let mut group_transform = CT_Transform2D::default();
    group_transform.offset = Some(CT_Point2D {
        x: points_to_emu(target_center.x - extent_width / 2.0)?,
        y: points_to_emu(target_center.y - extent_height / 2.0)?,
    });
    group_transform.extent = Some(CT_PositiveSize2D {
        cx: points_to_emu(extent_width)?,
        cy: points_to_emu(extent_height)?,
    });
    group_transform.child_offset = Some(CT_Point2D {
        x: points_to_emu(rect.x)?,
        y: points_to_emu(rect.y)?,
    });
    group_transform.child_extent = Some(CT_PositiveSize2D {
        cx: points_to_emu(rect.width)?,
        cy: points_to_emu(rect.height)?,
    });
    group_transform.rotation = Angle::from_degrees(rotation);
    group_transform.flip_vertical = flip_vertical;
    Ok(EditableAffine {
        group_transform,
        child_rotation: Angle::from_degrees(-theta.to_degrees()),
    })
}

fn wrap_last_shape_in_affine_group(
    presentation: &mut Presentation,
    slide_index: usize,
    transform: CT_Transform2D,
) -> Result<()> {
    let slide_count = presentation.slides.len();
    let slide = presentation
        .slides
        .get_mut(slide_index)
        .ok_or(Error::UnknownSlideIndex {
            index: slide_index,
            slide_count,
        })?;
    let tree = &mut Arc::make_mut(slide).slide.common_slide_data.shape_tree;
    let id = ShapeIdAllocator::scan(tree).allocate();
    let child = tree
        .children
        .pop()
        .ok_or_else(|| import_error(None, None, "affine PDF child shape is missing"))?;
    let mut group = CT_GroupShape::new_empty(id, &format!("PDF affine group {id}"));
    *group.group_transform_mut() = transform;
    group.children.push(child);
    tree.append_child(ShapeTreeChild::GroupShape(Box::new(group)));
    Ok(())
}

fn validate_limits(bytes: &[u8], mode: PdfImportMode, limits: PdfImportLimits) -> Result<()> {
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limits.max_input_bytes {
        return Err(import_error(
            None,
            None,
            format!(
                "PDF input length {} exceeds limit {}",
                bytes.len(),
                limits.max_input_bytes
            ),
        ));
    }
    if limits.max_pages == 0
        || limits.max_objects == 0
        || limits.max_decompressed_bytes == 0
        || limits.max_operations_per_page == 0
        || limits.max_pixels_per_page == 0
        || limits.max_shapes_per_page == 0
        || limits.max_diagnostics == 0
    {
        return Err(import_error(
            None,
            None,
            "PDF import limits must all be positive",
        ));
    }
    if let PdfImportMode::PreservedGraphic { dpi } = mode {
        finite_positive(dpi, 0, 0, "DPI")?;
    }
    Ok(())
}

fn import_error(page: Option<u32>, offset: Option<usize>, message: impl Into<String>) -> Error {
    Error::PdfImport {
        page,
        offset,
        message: message.into(),
    }
}

fn reject_active_content(document: &Document, object_limit: usize) -> Result<usize> {
    let work_limit = object_limit.saturating_mul(256);
    let mut scanner = ActiveContentScanner {
        document,
        seen: HashSet::new(),
        active_actions: HashSet::new(),
        work: 0,
        work_limit,
    };
    for id in document.objects.keys() {
        scanner.scan(
            &Object::Reference(*id),
            &format!("object {} {}", id.0, id.1),
            false,
            0,
        )?;
    }
    Ok(scanner.work)
}

struct ActiveContentScanner<'a> {
    document: &'a Document,
    seen: HashSet<(ObjectId, bool)>,
    active_actions: HashSet<ObjectId>,
    work: usize,
    work_limit: usize,
}

impl ActiveContentScanner<'_> {
    fn scan(
        &mut self,
        object: &Object,
        path: &str,
        action_context: bool,
        depth: usize,
    ) -> Result<()> {
        self.work = self.work.saturating_add(1);
        if self.work > self.work_limit {
            return Err(import_error(
                None,
                None,
                format!(
                    "active-content graph work exceeds limit {} at {path}",
                    self.work_limit
                ),
            ));
        }
        if depth > MAX_PDF_GRAPH_DEPTH {
            return Err(import_error(
                None,
                None,
                format!("active-content graph exceeds depth limit at {path}"),
            ));
        }
        match object {
            Object::Reference(id) => {
                if action_context {
                    if self.seen.contains(&(*id, true)) {
                        return Ok(());
                    }
                    if !self.active_actions.insert(*id) {
                        return Err(import_error(
                            None,
                            None,
                            format!("cyclic action reference at {path}"),
                        ));
                    }
                    let target = self.document.objects.get(id).ok_or_else(|| {
                        import_error(
                            None,
                            None,
                            format!(
                                "active-content reference failed at {path}: missing object {} {}",
                                id.0, id.1
                            ),
                        )
                    })?;
                    let result = self.scan(
                        target,
                        &format!("{path}/{} {}", id.0, id.1),
                        true,
                        depth + 1,
                    );
                    self.active_actions.remove(id);
                    result?;
                    self.seen.insert((*id, true));
                } else if self.seen.insert((*id, false)) {
                    let target = self.document.get_object(*id).map_err(|error| {
                        import_error(
                            None,
                            None,
                            format!("active-content reference failed at {path}: {error}"),
                        )
                    })?;
                    self.scan(
                        target,
                        &format!("{path}/{} {}", id.0, id.1),
                        false,
                        depth + 1,
                    )?;
                }
            }
            Object::Array(values) => {
                if action_context {
                    return Err(import_error(
                        None,
                        None,
                        format!("action at {path} must resolve to a dictionary"),
                    ));
                }
                for (index, value) in values.iter().enumerate() {
                    self.scan(value, &format!("{path}[{index}]"), false, depth + 1)?;
                }
            }
            Object::Dictionary(dictionary) => {
                self.scan_dictionary(dictionary, path, action_context, depth)?;
            }
            Object::Stream(stream) => {
                if action_context {
                    return Err(import_error(
                        None,
                        None,
                        format!("action at {path} must resolve to a dictionary"),
                    ));
                }
                self.scan_dictionary(&stream.dict, path, false, depth)?;
            }
            _ if action_context => {
                return Err(import_error(
                    None,
                    None,
                    format!("action at {path} must resolve to a dictionary"),
                ));
            }
            _ => {}
        }
        Ok(())
    }

    fn scan_dictionary(
        &mut self,
        dictionary: &Dictionary,
        path: &str,
        action_context: bool,
        depth: usize,
    ) -> Result<()> {
        if dictionary.has(b"AA") {
            return Err(import_error(
                None,
                None,
                format!("additional actions are unsupported at {path}/AA"),
            ));
        }
        let action_kind = dictionary.get(b"S").and_then(Object::as_name).ok();
        if dictionary.has(b"JS") || (action_context && action_kind == Some(b"JavaScript")) {
            return Err(import_error(
                None,
                None,
                format!("JavaScript active content is unsupported at {path}"),
            ));
        }
        if action_context {
            strict_uri_action_dictionary(dictionary, None, path)?;
        }
        let object_type = dictionary.get(b"Type").and_then(Object::as_name).ok();
        for (key, value) in dictionary.iter() {
            let nested_action = match key.as_slice() {
                b"OpenAction" => object_type == Some(b"Catalog"),
                b"A" => object_type == Some(b"Annot"),
                b"Next" => action_context,
                _ => false,
            };
            self.scan(
                value,
                &format!("{path}/{}", String::from_utf8_lossy(key)),
                nested_action,
                depth + 1,
            )?;
        }
        Ok(())
    }
}

fn strict_pages(
    document: &Document,
    max_pages: usize,
    max_objects: usize,
) -> Result<Vec<(u32, ObjectId)>> {
    enum Visit {
        Enter {
            id: ObjectId,
            parent: Option<ObjectId>,
            depth: usize,
        },
        Exit {
            id: ObjectId,
            declared_count: usize,
            first_page: usize,
        },
    }

    let root = document
        .trailer
        .get(b"Root")
        .and_then(Object::as_reference)
        .map_err(|_| import_error(None, None, "PDF trailer Root must be an indirect reference"))?;
    let catalog = document
        .get_dictionary(root)
        .map_err(|error| import_error(None, None, format!("PDF catalog object failed: {error}")))?;
    if catalog.get(b"Type").and_then(Object::as_name).ok() != Some(b"Catalog") {
        return Err(import_error(
            None,
            None,
            "PDF catalog must declare /Type /Catalog",
        ));
    }
    let pages_root = catalog
        .get(b"Pages")
        .and_then(Object::as_reference)
        .map_err(|_| {
            import_error(
                None,
                None,
                "PDF catalog Pages must be an indirect reference",
            )
        })?;

    let mut pages = Vec::new();
    let mut active = HashSet::new();
    let mut visited = HashSet::new();
    let mut work = 1usize;
    let mut stack = vec![Visit::Enter {
        id: pages_root,
        parent: None,
        depth: 0,
    }];
    while let Some(visit) = stack.pop() {
        match visit {
            Visit::Exit {
                id,
                declared_count,
                first_page,
            } => {
                let actual_count = pages.len().checked_sub(first_page).ok_or_else(|| {
                    import_error(None, None, "PDF page-tree count arithmetic underflow")
                })?;
                if actual_count != declared_count {
                    return Err(import_error(
                        None,
                        None,
                        format!(
                            "page-tree node {} {} declares Count {declared_count} but contains {actual_count} pages",
                            id.0, id.1
                        ),
                    ));
                }
                active.remove(&id);
                visited.insert(id);
            }
            Visit::Enter { id, parent, depth } => {
                if active.contains(&id) {
                    return Err(import_error(
                        None,
                        None,
                        format!("cyclic page tree at object {} {}", id.0, id.1),
                    ));
                }
                if visited.contains(&id) {
                    return Err(import_error(
                        None,
                        None,
                        format!("duplicate page-tree node at object {} {}", id.0, id.1),
                    ));
                }
                if depth > MAX_PDF_GRAPH_DEPTH {
                    return Err(import_error(
                        None,
                        None,
                        format!(
                            "page tree exceeds depth limit {} at object {} {}",
                            MAX_PDF_GRAPH_DEPTH, id.0, id.1
                        ),
                    ));
                }
                let dictionary = document.get_dictionary(id).map_err(|error| {
                    import_error(
                        None,
                        None,
                        format!("page-tree object {} {} failed: {error}", id.0, id.1),
                    )
                })?;
                match parent {
                    Some(expected) => {
                        let actual = dictionary
                            .get(b"Parent")
                            .and_then(Object::as_reference)
                            .map_err(|_| {
                                import_error(
                                    None,
                                    None,
                                    format!(
                                        "page-tree object {} {} Parent must be an indirect reference",
                                        id.0, id.1
                                    ),
                                )
                            })?;
                        if actual != expected {
                            return Err(import_error(
                                None,
                                None,
                                format!(
                                    "page-tree object {} {} Parent does not match its containing Pages node",
                                    id.0, id.1
                                ),
                            ));
                        }
                    }
                    None if dictionary.has(b"Parent") => {
                        return Err(import_error(
                            None,
                            None,
                            "root Pages node must not declare Parent",
                        ));
                    }
                    None => {}
                }
                let kind = dictionary
                    .get(b"Type")
                    .and_then(Object::as_name)
                    .map_err(|_| {
                        import_error(
                            None,
                            None,
                            format!(
                                "page-tree object {} {} must declare a name Type",
                                id.0, id.1
                            ),
                        )
                    })?;
                match kind {
                    b"Page" => {
                        if pages.len() >= max_pages {
                            return Err(import_error(
                                None,
                                None,
                                format!("PDF page count exceeds limit {max_pages}"),
                            ));
                        }
                        let number = u32::try_from(pages.len() + 1).map_err(|_| {
                            import_error(None, None, "PDF page number exceeds u32 range")
                        })?;
                        pages.push((number, id));
                        visited.insert(id);
                    }
                    b"Pages" => {
                        let declared_count = dictionary
                            .get(b"Count")
                            .and_then(Object::as_i64)
                            .ok()
                            .and_then(|value| usize::try_from(value).ok())
                            .ok_or_else(|| {
                                import_error(
                                    None,
                                    None,
                                    format!(
                                        "Pages node {} {} Count must be a nonnegative integer",
                                        id.0, id.1
                                    ),
                                )
                            })?;
                        let kids =
                            dictionary
                                .get(b"Kids")
                                .and_then(Object::as_array)
                                .map_err(|_| {
                                    import_error(
                                        None,
                                        None,
                                        format!(
                                            "Pages node {} {} Kids must be an array",
                                            id.0, id.1
                                        ),
                                    )
                                })?;
                        work = work.checked_add(kids.len()).ok_or_else(|| {
                            import_error(None, None, "page-tree work count overflow")
                        })?;
                        if work > max_objects {
                            return Err(import_error(
                                None,
                                None,
                                format!("page-tree work exceeds object limit {max_objects}"),
                            ));
                        }
                        active.insert(id);
                        stack.push(Visit::Exit {
                            id,
                            declared_count,
                            first_page: pages.len(),
                        });
                        for kid in kids.iter().rev() {
                            let kid = kid.as_reference().map_err(|_| {
                                import_error(
                                    None,
                                    None,
                                    format!(
                                        "Pages node {} {} Kids members must be indirect references",
                                        id.0, id.1
                                    ),
                                )
                            })?;
                            stack.push(Visit::Enter {
                                id: kid,
                                parent: Some(id),
                                depth: depth + 1,
                            });
                        }
                    }
                    other => {
                        return Err(import_error(
                            None,
                            None,
                            format!(
                                "page-tree object {} {} has unsupported Type /{}",
                                id.0,
                                id.1,
                                String::from_utf8_lossy(other)
                            ),
                        ));
                    }
                }
            }
        }
    }
    if pages.is_empty() {
        return Err(import_error(None, None, "PDF contains no pages"));
    }
    Ok(pages)
}

fn strict_page_content(
    document: &Document,
    page_id: ObjectId,
    page: u32,
    max_objects: usize,
    max_decompressed: usize,
) -> Result<Vec<u8>> {
    let dictionary = document.get_dictionary(page_id).map_err(|error| {
        import_error(
            Some(page),
            None,
            format!("page dictionary failed while resolving Contents: {error}"),
        )
    })?;
    let Ok(contents) = dictionary.get(b"Contents") else {
        return Ok(Vec::new());
    };
    let mut resolver = ResourceReferenceResolver::new(max_objects);
    let mut ids = Vec::new();
    match contents {
        Object::Reference(id) => ids.push(*id),
        Object::Array(values) => {
            resolver.charge_many(page, values.len())?;
            if values.len() > max_objects {
                return Err(import_error(
                    Some(page),
                    None,
                    format!("page Contents count exceeds object limit {max_objects}"),
                ));
            }
            ids.reserve(values.len());
            for (index, value) in values.iter().enumerate() {
                let id = value.as_reference().map_err(|_| {
                    import_error(
                        Some(page),
                        None,
                        format!("page Contents array member {index} must be an indirect reference"),
                    )
                })?;
                ids.push(id);
            }
        }
        _ => {
            return Err(import_error(
                Some(page),
                None,
                "page Contents must be an indirect stream reference or an array of indirect stream references",
            ));
        }
    }

    let mut content = Vec::new();
    for (index, id) in ids.into_iter().enumerate() {
        let label = format!("page Contents stream {index}");
        let (_, object) = resolver.resolve(document, id, page, &label)?;
        let stream = object.as_stream().map_err(|_| {
            import_error(Some(page), None, format!("{label} must reference a stream"))
        })?;
        let remaining = max_decompressed.saturating_sub(content.len());
        let decoded = strict_content_stream_data(stream, page, index, remaining)?;
        let retained = content
            .len()
            .checked_add(decoded.len())
            .and_then(|length| length.checked_add(1))
            .ok_or_else(|| import_error(Some(page), None, "page content byte count overflow"))?;
        if retained > max_decompressed {
            return Err(import_error(
                Some(page),
                None,
                format!("page content bytes exceed limit {max_decompressed}"),
            ));
        }
        content.extend_from_slice(&decoded);
        content.push(b'\n');
    }
    Ok(content)
}

fn strict_content_stream_data(
    stream: &Stream,
    page: u32,
    index: usize,
    limit: usize,
) -> Result<Vec<u8>> {
    strict_flate_stream_data(
        stream,
        Some(page),
        &format!("page Contents stream {index}"),
        limit,
    )
}

fn strict_flate_stream_data(
    stream: &Stream,
    page: Option<u32>,
    label: &str,
    limit: usize,
) -> Result<Vec<u8>> {
    if stream.dict.has(b"DecodeParms") {
        return Err(import_error(
            page,
            None,
            format!("{label} DecodeParms are unsupported"),
        ));
    }
    let filters = match stream.dict.get(b"Filter") {
        Err(_) => Vec::new(),
        Ok(Object::Name(name)) => vec![name.as_slice()],
        Ok(Object::Array(values)) => values
            .iter()
            .map(|value| {
                value.as_name().map_err(|_| {
                    import_error(page, None, format!("{label} Filter members must be names"))
                })
            })
            .collect::<Result<Vec<_>>>()?,
        Ok(_) => {
            return Err(import_error(
                page,
                None,
                format!("{label} Filter must be a name or array of names"),
            ));
        }
    };
    if let Some(filter) = filters.iter().find(|filter| **filter != b"FlateDecode") {
        return Err(import_error(
            page,
            None,
            format!(
                "{label} uses unsupported Filter /{}",
                String::from_utf8_lossy(filter)
            ),
        ));
    }
    if filters.is_empty() {
        if stream.content.len() > limit {
            return Err(import_error(
                page,
                None,
                format!("{label} bytes exceed limit {limit}"),
            ));
        }
        return Ok(stream.content.clone());
    }
    let mut decoded =
        miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&stream.content, limit).map_err(
            |error| import_error(page, None, format!("{label} FlateDecode failed: {error:?}")),
        )?;
    for _ in filters.iter().skip(1) {
        decoded = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&decoded, limit)
            .map_err(|error| {
                import_error(page, None, format!("{label} FlateDecode failed: {error:?}"))
            })?;
    }
    Ok(decoded)
}

enum AnnotationDecision {
    Skip,
    Diagnostic(String),
    Link { rect: Rect, uri: String },
}

fn strict_annotation_ids(
    document: &Document,
    page_id: ObjectId,
    page: u32,
    retained_elements: usize,
    limits: PdfImportLimits,
    resolver: &mut ResourceReferenceResolver,
) -> Result<Vec<ObjectId>> {
    let dictionary = document.get_dictionary(page_id).map_err(|error| {
        import_error(
            Some(page),
            None,
            format!("page dictionary failed while resolving Annots: {error}"),
        )
    })?;
    let Ok(mut annotations) = dictionary.get(b"Annots") else {
        return Ok(Vec::new());
    };
    if let Object::Reference(id) = annotations {
        annotations = resolver.resolve(document, *id, page, "page Annots")?.1;
    }
    let annotations = annotations
        .as_array()
        .map_err(|_| import_error(Some(page), None, "page Annots must resolve to an array"))?;
    resolver.charge_many(page, annotations.len())?;
    let retained = retained_elements
        .checked_add(annotations.len())
        .ok_or_else(|| import_error(Some(page), None, "annotation shape count overflow"))?;
    if retained > limits.max_shapes_per_page {
        return Err(import_error(
            Some(page),
            None,
            format!(
                "page annotation count {} exceeds remaining shape limit {}",
                annotations.len(),
                limits.max_shapes_per_page.saturating_sub(retained_elements)
            ),
        ));
    }
    let mut ids = Vec::with_capacity(annotations.len());
    for (index, annotation) in annotations.iter().enumerate() {
        ids.push(annotation.as_reference().map_err(|_| {
            import_error(
                Some(page),
                None,
                format!("page Annots member {index} must be an indirect reference"),
            )
        })?);
    }
    Ok(ids)
}

fn strict_annotation(
    document: &Document,
    page: &NormalizedPage,
    index: usize,
    id: ObjectId,
    resolver: &mut ResourceReferenceResolver,
    seen: &mut HashSet<ObjectId>,
) -> Result<AnnotationDecision> {
    let label = format!("annotation {index}");
    let (terminal, object) = resolver.resolve(document, id, page.number, &label)?;
    if !seen.insert(terminal) {
        return Err(import_error(
            Some(page.number),
            None,
            format!(
                "page Annots repeats annotation target {} {}",
                terminal.0, terminal.1
            ),
        ));
    }
    let annotation = object.as_dict().map_err(|_| {
        import_error(
            Some(page.number),
            None,
            format!("{label} must reference a dictionary"),
        )
    })?;
    if annotation.get(b"Type").and_then(Object::as_name).ok() != Some(b"Annot") {
        return Err(import_error(
            Some(page.number),
            None,
            format!("{label} dictionary must declare /Type /Annot"),
        ));
    }
    let subtype = annotation
        .get(b"Subtype")
        .and_then(Object::as_name)
        .map_err(|_| {
            import_error(
                Some(page.number),
                None,
                format!("{label} dictionary must declare a name Subtype"),
            )
        })?;
    if subtype != b"Link" {
        return Ok(AnnotationDecision::Skip);
    }
    let values = annotation
        .get(b"Rect")
        .and_then(Object::as_array)
        .map_err(|error| {
            import_error(
                Some(page.number),
                None,
                format!("link annotation {index} rectangle failed: {error}"),
            )
        })?;
    let rect = page.geometry.rect(
        array_numbers(values, 4, page.number, index)?
            .try_into()
            .map_err(|_| {
                import_error(
                    Some(page.number),
                    None,
                    "link rectangle requires four values",
                )
            })?,
    );
    checked_rect(rect, page.number, index, "link rectangle")?;
    let Ok(mut action) = annotation.get(b"A") else {
        return Ok(AnnotationDecision::Diagnostic(
            "link annotation without a URI action was dropped".to_owned(),
        ));
    };
    if let Object::Reference(id) = action {
        action = resolver
            .resolve(
                document,
                *id,
                page.number,
                &format!("link annotation {index} action"),
            )?
            .1;
    }
    let action = action.as_dict().map_err(|_| {
        import_error(
            Some(page.number),
            None,
            format!("link annotation {index} action must resolve to a dictionary"),
        )
    })?;
    let uri = strict_uri_action_dictionary(
        action,
        Some(page.number),
        &format!("link annotation {index} action"),
    )?;
    Ok(AnnotationDecision::Link { rect, uri })
}

fn page_geometry(document: &Document, page_id: ObjectId, page: u32) -> Result<PageGeometry> {
    let media = inherited(document, page_id, b"MediaBox")?
        .ok_or_else(|| import_error(Some(page), None, "page has no inherited MediaBox"))?;
    let crop = inherited(document, page_id, b"CropBox")?.unwrap_or(media);
    let values = crop
        .as_array()
        .map_err(|error| import_error(Some(page), None, format!("page box failed: {error}")))?;
    let values: [f64; 4] = array_numbers(values, 4, page, 0)?
        .try_into()
        .map_err(|_| import_error(Some(page), None, "page box requires four numbers"))?;
    if values[2] <= values[0] || values[3] <= values[1] {
        return Err(import_error(
            Some(page),
            None,
            "page box has non-positive dimensions",
        ));
    }
    let rotation = inherited(document, page_id, b"Rotate")?
        .map(object_number)
        .transpose()
        .map_err(|error| import_error(Some(page), None, error))?
        .unwrap_or(0.0);
    if rotation.fract() != 0.0 {
        return Err(import_error(
            Some(page),
            None,
            "page rotation must be an integer",
        ));
    }
    let rotation = (rotation as i32).rem_euclid(360);
    if !matches!(rotation, 0 | 90 | 180 | 270) {
        return Err(import_error(
            Some(page),
            None,
            format!("unsupported page rotation {rotation}"),
        ));
    }
    let raw_width = values[2] - values[0];
    let raw_height = values[3] - values[1];
    if !raw_width.is_finite() || !raw_height.is_finite() {
        return Err(import_error(
            Some(page),
            None,
            "page dimensions produce non-finite geometry",
        ));
    }
    let (width, height) = if matches!(rotation, 90 | 270) {
        (raw_height, raw_width)
    } else {
        (raw_width, raw_height)
    };
    Ok(PageGeometry {
        left: values[0],
        bottom: values[1],
        right: values[2],
        top: values[3],
        rotation,
        width,
        height,
    })
}

fn inherited<'a>(
    document: &'a Document,
    mut id: ObjectId,
    key: &[u8],
) -> Result<Option<&'a Object>> {
    let mut seen = HashSet::new();
    loop {
        if !seen.insert(id) {
            return Err(import_error(
                None,
                None,
                format!("cyclic page tree at object {} {}", id.0, id.1),
            ));
        }
        let dictionary = document.get_dictionary(id).map_err(|error| {
            import_error(None, None, format!("page tree object failed: {error}"))
        })?;
        if let Ok(value) = dictionary.get(key) {
            let (_, value) = document.dereference(value).map_err(|error| {
                import_error(None, None, format!("inherited value failed: {error}"))
            })?;
            return Ok(Some(value));
        }
        match dictionary.get(b"Parent").and_then(Object::as_reference) {
            Ok(parent) => id = parent,
            Err(_) => return Ok(None),
        }
    }
}

fn page_fonts(
    document: &Document,
    caches: &mut FontEncodingCaches,
    fonts: &BTreeMap<Vec<u8>, ObjectId>,
    operations: &[Operation],
    widths: &HashMap<ObjectId, Option<SimpleFontWidths>>,
) -> Result<BTreeMap<Vec<u8>, PageFont>> {
    let requested = text_strings_by_font(operations);
    fonts
        .iter()
        .map(|(name, id)| {
            let dictionary = document.get_dictionary(*id).map_err(|error| {
                import_error(
                    None,
                    None,
                    format!(
                        "resolved font /{} failed: {error}",
                        String::from_utf8_lossy(name)
                    ),
                )
            })?;
            let shaping_family = dictionary
                .get(b"BaseFont")
                .and_then(Object::as_name)
                .map(|name| String::from_utf8_lossy(name).into_owned())
                .unwrap_or_else(|_| String::from_utf8_lossy(name).into_owned());
            let display_family = shaping_family
                .split_once('+')
                .map_or(shaping_family.as_str(), |(_, family)| family)
                .to_owned();
            let has_to_unicode = dictionary.has(b"ToUnicode");
            let (encoding, encoding_supported) = if has_to_unicode {
                ("ToUnicode".to_owned(), true)
            } else {
                match dictionary.get(b"Encoding") {
                    Ok(Object::Name(name)) => {
                        let label = String::from_utf8_lossy(name).into_owned();
                        let identity = matches!(name.as_slice(), b"Identity-H" | b"Identity-V");
                        let supported = matches!(
                            name.as_slice(),
                            b"StandardEncoding"
                                | b"WinAnsiEncoding"
                                | b"MacRomanEncoding"
                                | b"MacExpertEncoding"
                        );
                        let supported = supported && !identity;
                        (label, supported)
                    }
                    Ok(Object::Dictionary(_)) | Ok(Object::Reference(_)) => {
                        ("custom encoding dictionary".to_owned(), false)
                    }
                    Ok(other) => (format!("{} encoding object", other.enum_variant()), false),
                    Err(_) => ("implicit StandardEncoding".to_owned(), true),
                }
            };
            let mut decoded = BTreeMap::new();
            if encoding_supported {
                let decoder = if has_to_unicode {
                    let stream_id = dictionary
                        .get(b"ToUnicode")
                        .and_then(Object::as_reference)
                        .map_err(|_| {
                            import_error(
                                None,
                                None,
                                "prevalidated font ToUnicode must reference its terminal stream",
                            )
                        })?;
                    if let Some(decoder) = caches.to_unicode_by_stream.get(&stream_id) {
                        Arc::clone(decoder)
                    } else {
                        let limit = document
                            .get_object(stream_id)
                            .and_then(Object::as_stream)
                            .map_err(|error| {
                                import_error(
                                    None,
                                    None,
                                    format!("prevalidated font ToUnicode failed: {error}"),
                                )
                            })?
                            .content
                            .len()
                            .max(1);
                        let mut to_unicode_dictionary = dictionary.clone();
                        to_unicode_dictionary.remove(b"Encoding");
                        let parsed = to_unicode_dictionary
                            .get_font_encoding_with_limit(document, limit)
                            .map_err(|error| {
                                import_error(
                                    None,
                                    None,
                                    format!("font encoding `{encoding}` failed: {error}"),
                                )
                            })?;
                        let lopdf::Encoding::UnicodeMapEncoding(map) = parsed else {
                            return Err(import_error(
                                None,
                                None,
                                "font ToUnicode stream does not contain a valid Unicode CMap",
                            ));
                        };
                        let decoder = Arc::new(CachedFontEncoding::Unicode(
                            lopdf::Encoding::UnicodeMapEncoding(map),
                        ));
                        caches.to_unicode_parses =
                            caches.to_unicode_parses.checked_add(1).ok_or_else(|| {
                                import_error(None, None, "ToUnicode parse count overflow")
                            })?;
                        caches
                            .to_unicode_by_stream
                            .insert(stream_id, Arc::clone(&decoder));
                        decoder
                    }
                } else if let Some(decoder) = caches.ordinary_by_font.get(id) {
                    Arc::clone(decoder)
                } else {
                    let parsed = dictionary
                        .get_font_encoding_with_limit(document, 1)
                        .map_err(|error| {
                            import_error(
                                None,
                                None,
                                format!("font encoding `{encoding}` failed: {error}"),
                            )
                        })?;
                    let decoder = match parsed {
                        lopdf::Encoding::UnicodeMapEncoding(map) => {
                            CachedFontEncoding::Unicode(lopdf::Encoding::UnicodeMapEncoding(map))
                        }
                        other => {
                            let mut values = Vec::with_capacity(256);
                            for byte in 0_u8..=u8::MAX {
                                values.push(Document::decode_text(&other, &[byte]).map_err(
                                    |error| {
                                        import_error(
                                            None,
                                            None,
                                            format!("font encoding `{encoding}` failed: {error}"),
                                        )
                                    },
                                )?);
                            }
                            CachedFontEncoding::SingleByte(values)
                        }
                    };
                    let decoder = Arc::new(decoder);
                    caches.ordinary_by_font.insert(*id, Arc::clone(&decoder));
                    decoder
                };
                if encoding_supported && let Some(values) = requested.get(name) {
                    for value in values {
                        decoded.insert(value.clone(), decoder.decode(value));
                    }
                }
            }
            let composite_widths_supported =
                dictionary.get(b"Subtype").and_then(Object::as_name).ok() != Some(b"Type0");
            let widths = widths.get(id).cloned().ok_or_else(|| {
                import_error(
                    None,
                    None,
                    format!(
                        "font /{} was not strictly prevalidated",
                        String::from_utf8_lossy(name)
                    ),
                )
            })?;
            Ok((
                name.clone(),
                PageFont {
                    display_family,
                    shaping_family,
                    encoding,
                    encoding_supported,
                    decoded,
                    widths,
                    composite_widths_supported,
                },
            ))
        })
        .collect()
}

fn strict_font_widths(
    document: &Document,
    dictionary: &Dictionary,
    resolver: &mut ResourceReferenceResolver,
    missing_width: f64,
) -> Result<Option<SimpleFontWidths>> {
    let Ok(mut values) = dictionary.get(b"Widths") else {
        return Ok(None);
    };
    if let Object::Reference(id) = values {
        values = resolver.resolve(document, *id, 0, "font Widths")?.1;
    }
    let first_char = dictionary
        .get(b"FirstChar")
        .and_then(Object::as_i64)
        .map_err(|error| import_error(None, None, format!("font FirstChar failed: {error}")))?;
    let values = values
        .as_array()
        .map_err(|_| import_error(None, None, "font Widths must resolve to an array"))?;
    resolver.charge_many(0, values.len())?;
    if let Ok(last_char) = dictionary.get(b"LastChar") {
        let last_char = last_char
            .as_i64()
            .map_err(|_| import_error(None, None, "font LastChar must be an integer"))?;
        let expected = last_char
            .checked_sub(first_char)
            .and_then(|span| span.checked_add(1))
            .and_then(|count| usize::try_from(count).ok())
            .ok_or_else(|| import_error(None, None, "font character range is invalid"))?;
        if expected != values.len() {
            return Err(import_error(
                None,
                None,
                format!(
                    "font Widths length {} does not match FirstChar and LastChar range {expected}",
                    values.len()
                ),
            ));
        }
    }
    let mut widths = Vec::with_capacity(values.len());
    for value in values {
        let width = object_number(value)
            .map_err(|message| import_error(None, None, format!("font Widths {message}")))?;
        if !width.is_finite() || width < 0.0 {
            return Err(import_error(
                None,
                None,
                "font Widths entries must be finite and nonnegative",
            ));
        }
        widths.push(width);
    }
    Ok(Some(SimpleFontWidths {
        first_char,
        widths,
        missing_width,
    }))
}

fn text_strings_by_font(operations: &[Operation]) -> BTreeMap<Vec<u8>, HashSet<Vec<u8>>> {
    let mut result = BTreeMap::<Vec<u8>, HashSet<Vec<u8>>>::new();
    let mut font = None::<Vec<u8>>;
    let mut stack = Vec::new();
    for operation in operations {
        match operation.operator.as_str() {
            "q" => stack.push(font.clone()),
            "Q" => font = stack.pop().unwrap_or(None),
            "Tf" => {
                font = operation
                    .operands
                    .first()
                    .and_then(|value| value.as_name().ok())
                    .map(Vec::from);
            }
            "Tj" | "'" => {
                if let (Some(font), Some(value)) = (
                    font.as_ref(),
                    operation
                        .operands
                        .first()
                        .and_then(|value| value.as_str().ok()),
                ) {
                    result
                        .entry(font.clone())
                        .or_default()
                        .insert(value.to_vec());
                }
            }
            "\"" => {
                if let (Some(font), Some(value)) = (
                    font.as_ref(),
                    operation
                        .operands
                        .get(2)
                        .and_then(|value| value.as_str().ok()),
                ) {
                    result
                        .entry(font.clone())
                        .or_default()
                        .insert(value.to_vec());
                }
            }
            "TJ" => {
                if let (Some(font), Some(array)) = (
                    font.as_ref(),
                    operation
                        .operands
                        .first()
                        .and_then(|value| value.as_array().ok()),
                ) {
                    let values = result.entry(font.clone()).or_default();
                    for value in array.iter().filter_map(|value| value.as_str().ok()) {
                        values.insert(value.to_vec());
                    }
                }
            }
            _ => {}
        }
    }
    result
}

fn embedded_fonts<'a>(
    document: &mut Document,
    pages: impl Iterator<Item = &'a BTreeMap<Vec<u8>, ObjectId>>,
    limit: usize,
    max_objects: usize,
) -> Result<EmbeddedFonts> {
    let mut result = Vec::new();
    let mut retained = 0usize;
    let mut retained_streams = HashSet::new();
    let mut retained_font_streams = HashSet::new();
    let mut visited_fonts = HashSet::new();
    let mut decoded_maps = Vec::<(ObjectId, Vec<u8>)>::new();
    let mut normalized_maps = Vec::<(ObjectId, ObjectId)>::new();
    let mut normalized_encodings = Vec::<(ObjectId, Object)>::new();
    let mut font_widths = HashMap::new();
    let mut resolver = ResourceReferenceResolver::new(max_objects);
    for fonts in pages {
        for id in fonts.values().copied() {
            if !visited_fonts.insert(id) {
                continue;
            }
            let dictionary = document.get_dictionary(id).map_err(|error| {
                import_error(
                    None,
                    None,
                    format!("resolved font dictionary failed: {error}"),
                )
            })?;
            let family = match dictionary.get(b"BaseFont") {
                Ok(Object::Name(name)) => String::from_utf8_lossy(name).into_owned(),
                Ok(_) => {
                    return Err(import_error(
                        None,
                        None,
                        "font BaseFont must be a name when present",
                    ));
                }
                Err(_) => "Embedded PDF font".to_owned(),
            };
            if let Ok(value) = dictionary.get(b"Encoding") {
                match value {
                    Object::Name(_) => {}
                    Object::Dictionary(dictionary) => {
                        resolver.charge_many(0, dictionary.len())?;
                    }
                    Object::Reference(reference) => {
                        let resolved = resolver
                            .resolve(document, *reference, 0, "font Encoding")?
                            .1;
                        match resolved {
                            Object::Name(_) => {}
                            Object::Dictionary(dictionary) => {
                                resolver.charge_many(0, dictionary.len())?;
                            }
                            _ => {
                                return Err(import_error(
                                    None,
                                    None,
                                    "font Encoding reference must resolve to a name or dictionary",
                                ));
                            }
                        }
                        normalized_encodings.push((id, resolved.clone()));
                    }
                    _ => {
                        return Err(import_error(
                            None,
                            None,
                            "font Encoding must be a name or dictionary",
                        ));
                    }
                }
            }
            if let Ok(value) = dictionary.get(b"ToUnicode") {
                let reference = value.as_reference().map_err(|_| {
                    import_error(
                        None,
                        None,
                        "font ToUnicode must be an indirect stream reference",
                    )
                })?;
                let (stream_id, object) =
                    resolver.resolve(document, reference, 0, "font ToUnicode")?;
                let stream = object.as_stream().map_err(|_| {
                    import_error(None, None, "font ToUnicode must reference a stream")
                })?;
                normalized_maps.push((id, stream_id));
                if retained_streams.insert(stream_id) {
                    let remaining = limit.saturating_sub(retained);
                    let data =
                        strict_flate_stream_data(stream, None, "font ToUnicode stream", remaining)?;
                    retained = retained.checked_add(data.len()).ok_or_else(|| {
                        import_error(None, None, "font encoding byte count overflow")
                    })?;
                    decoded_maps.push((stream_id, data));
                }
            }
            let mut missing_width = 0.0;
            let mut font_file = None;
            if let Ok(mut descriptor) = dictionary.get(b"FontDescriptor") {
                if let Object::Reference(reference) = descriptor {
                    descriptor = resolver
                        .resolve(document, *reference, 0, "font descriptor")?
                        .1;
                }
                let descriptor = descriptor.as_dict().map_err(|_| {
                    import_error(None, None, "font descriptor must resolve to a dictionary")
                })?;
                if let Ok(value) = descriptor.get(b"Type")
                    && value.as_name().ok() != Some(b"FontDescriptor")
                {
                    return Err(import_error(
                        None,
                        None,
                        "font descriptor Type must be /FontDescriptor",
                    ));
                }
                if let Ok(value) = descriptor.get(b"MissingWidth") {
                    missing_width = object_number(value).map_err(|message| {
                        import_error(None, None, format!("font MissingWidth {message}"))
                    })?;
                    if missing_width < 0.0 {
                        return Err(import_error(
                            None,
                            None,
                            "font MissingWidth must be finite and nonnegative",
                        ));
                    }
                }
                if let Ok(value) = descriptor.get(b"FontFile2") {
                    let reference = value.as_reference().map_err(|_| {
                        import_error(
                            None,
                            None,
                            "font FontFile2 must be an indirect stream reference",
                        )
                    })?;
                    let (stream_id, object) =
                        resolver.resolve(document, reference, 0, "font FontFile2")?;
                    let stream = object.as_stream().map_err(|_| {
                        import_error(None, None, "font FontFile2 must reference a stream")
                    })?;
                    font_file = Some((stream_id, stream));
                }
            }
            font_widths.insert(
                id,
                strict_font_widths(document, dictionary, &mut resolver, missing_width)?,
            );
            if dictionary.get(b"Subtype").and_then(Object::as_name).ok() != Some(b"TrueType") {
                continue;
            }
            let Some((stream_id, stream)) = font_file else {
                continue;
            };
            if !retained_font_streams.insert(stream_id) {
                continue;
            }
            let remaining = limit.saturating_sub(retained);
            let data =
                strict_flate_stream_data(stream, None, "embedded TrueType font stream", remaining)?;
            retained = retained
                .checked_add(data.len())
                .ok_or_else(|| import_error(None, None, "embedded font byte count overflow"))?;
            if retained > limit {
                return Err(import_error(
                    None,
                    None,
                    format!("embedded font bytes exceed limit {limit}"),
                ));
            }
            result.push(FontFile { family, data });
        }
    }
    for (id, data) in decoded_maps {
        let stream = document
            .get_object_mut(id)
            .and_then(Object::as_stream_mut)
            .map_err(|error| {
                import_error(None, None, format!("ToUnicode cache failed: {error}"))
            })?;
        stream.content = data;
        stream.dict.remove(b"Filter");
        stream.dict.remove(b"DecodeParms");
        stream.dict.set("Length", stream.content.len() as i64);
    }
    for (font_id, stream_id) in normalized_maps {
        document
            .get_dictionary_mut(font_id)
            .map_err(|error| {
                import_error(None, None, format!("ToUnicode font cache failed: {error}"))
            })?
            .set("ToUnicode", stream_id);
    }
    for (font_id, encoding) in normalized_encodings {
        document
            .get_dictionary_mut(font_id)
            .map_err(|error| {
                import_error(None, None, format!("font Encoding cache failed: {error}"))
            })?
            .set("Encoding", encoding);
    }
    Ok(EmbeddedFonts {
        files: result,
        retained_bytes: retained,
        widths: font_widths,
    })
}

#[derive(Debug)]
struct ResolvedPageResources {
    fonts: BTreeMap<Vec<u8>, ObjectId>,
    xobjects: BTreeMap<Vec<u8>, ObjectId>,
}

fn resolve_page_resources(
    document: &Document,
    page_id: ObjectId,
    page: u32,
    max_objects: usize,
) -> Result<ResolvedPageResources> {
    let mut resolver = ResourceReferenceResolver::new(max_objects);
    let Some(resources) = inherited_page_resources(document, page_id, page, &mut resolver)? else {
        return Ok(ResolvedPageResources {
            fonts: BTreeMap::new(),
            xobjects: BTreeMap::new(),
        });
    };
    let fonts = font_entries(document, resources, page, &mut resolver)?;
    let xobjects =
        xobjects_from_resources(document, std::iter::once(resources), page, &mut resolver)?;
    Ok(ResolvedPageResources { fonts, xobjects })
}

fn xobjects_from_resources<'a>(
    document: &'a Document,
    resources: impl Iterator<Item = &'a Dictionary>,
    page: u32,
    resolver: &mut ResourceReferenceResolver,
) -> Result<BTreeMap<Vec<u8>, ObjectId>> {
    enum Visit {
        Enter { id: ObjectId, depth: usize },
        Exit(ObjectId),
    }

    let mut values = BTreeMap::new();
    let mut visited = HashSet::new();
    let mut active = HashSet::new();
    let mut stack = Vec::new();
    for resources in resources {
        let entries = xobject_entries(document, resources, page, resolver)?;
        for (name, id) in entries {
            values.entry(name).or_insert(id);
            stack.push(Visit::Enter { id, depth: 0 });
        }
    }
    while let Some(visit) = stack.pop() {
        match visit {
            Visit::Exit(id) => {
                active.remove(&id);
                visited.insert(id);
            }
            Visit::Enter { id, depth } => {
                if active.contains(&id) {
                    return Err(import_error(
                        Some(page),
                        None,
                        format!("cyclic XObject resource graph at object {} {}", id.0, id.1),
                    ));
                }
                if visited.contains(&id) {
                    continue;
                }
                if depth > MAX_PDF_GRAPH_DEPTH {
                    return Err(import_error(
                        Some(page),
                        None,
                        format!(
                            "XObject resource graph exceeds depth limit {} at object {} {}",
                            MAX_PDF_GRAPH_DEPTH, id.0, id.1
                        ),
                    ));
                }
                let stream = document
                    .objects
                    .get(&id)
                    .and_then(|object| object.as_stream().ok())
                    .ok_or_else(|| {
                        import_error(
                            Some(page),
                            None,
                            format!("validated XObject {} {} is not a stream", id.0, id.1),
                        )
                    })?;
                let subtype = stream
                    .dict
                    .get(b"Subtype")
                    .and_then(Object::as_name)
                    .map_err(|_| {
                        import_error(
                            Some(page),
                            None,
                            format!("validated XObject {} {} has no name Subtype", id.0, id.1),
                        )
                    })?;
                active.insert(id);
                stack.push(Visit::Exit(id));
                if subtype == b"Form"
                    && let Ok(nested) = stream.dict.get(b"Resources")
                {
                    let nested = strict_resource_dictionary(
                        document,
                        nested,
                        page,
                        "Form Resources",
                        resolver,
                    )?;
                    let entries = xobject_entries(document, nested, page, resolver)?;
                    for (_, child) in entries.into_iter().rev() {
                        stack.push(Visit::Enter {
                            id: child,
                            depth: depth + 1,
                        });
                    }
                }
            }
        }
    }
    Ok(values)
}

struct ResourceReferenceResolver {
    resolved: HashMap<ObjectId, ObjectId>,
    active: HashSet<ObjectId>,
    work: usize,
    max_work: usize,
}

impl ResourceReferenceResolver {
    fn new(max_work: usize) -> Self {
        Self {
            resolved: HashMap::new(),
            active: HashSet::new(),
            work: 0,
            max_work,
        }
    }

    fn charge(&mut self, page: u32) -> Result<()> {
        self.charge_many(page, 1)
    }

    fn charge_many(&mut self, page: u32, count: usize) -> Result<()> {
        self.work = self
            .work
            .checked_add(count)
            .ok_or_else(|| import_error(Some(page), None, "resource graph work overflow"))?;
        if self.work > self.max_work {
            return Err(import_error(
                Some(page),
                None,
                format!("resource graph work exceeds object limit {}", self.max_work),
            ));
        }
        Ok(())
    }

    fn resolve<'a>(
        &mut self,
        document: &'a Document,
        start: ObjectId,
        page: u32,
        label: &str,
    ) -> Result<(ObjectId, &'a Object)> {
        if let Some(terminal) = self.resolved.get(&start).copied() {
            let object = document.objects.get(&terminal).ok_or_else(|| {
                import_error(
                    Some(page),
                    None,
                    format!(
                        "cached {label} target is missing object {} {}",
                        terminal.0, terminal.1
                    ),
                )
            })?;
            return Ok((terminal, object));
        }

        let mut chain = Vec::new();
        let mut id = start;
        let terminal = loop {
            if let Some(terminal) = self.resolved.get(&id).copied() {
                break terminal;
            }
            if !self.active.insert(id) {
                for member in &chain {
                    self.active.remove(member);
                }
                return Err(import_error(
                    Some(page),
                    None,
                    format!("cyclic {label} reference at object {} {}", id.0, id.1),
                ));
            }
            chain.push(id);
            if chain.len() > MAX_PDF_GRAPH_DEPTH {
                for member in &chain {
                    self.active.remove(member);
                }
                return Err(import_error(
                    Some(page),
                    None,
                    format!(
                        "{label} reference exceeds depth limit {}",
                        MAX_PDF_GRAPH_DEPTH
                    ),
                ));
            }
            if let Err(error) = self.charge(page) {
                for member in &chain {
                    self.active.remove(member);
                }
                return Err(error);
            }
            let Some(object) = document.objects.get(&id) else {
                for member in &chain {
                    self.active.remove(member);
                }
                return Err(import_error(
                    Some(page),
                    None,
                    format!("{label} reference targets missing object {} {}", id.0, id.1),
                ));
            };
            match object {
                Object::Reference(next) => id = *next,
                _ => break id,
            }
        };
        for member in chain {
            self.active.remove(&member);
            self.resolved.insert(member, terminal);
        }
        let object = document.objects.get(&terminal).ok_or_else(|| {
            import_error(
                Some(page),
                None,
                format!(
                    "resolved {label} target is missing object {} {}",
                    terminal.0, terminal.1
                ),
            )
        })?;
        Ok((terminal, object))
    }
}

fn inherited_page_resources<'a>(
    document: &'a Document,
    page_id: ObjectId,
    page: u32,
    resolver: &mut ResourceReferenceResolver,
) -> Result<Option<&'a Dictionary>> {
    let mut seen = HashSet::new();
    let mut current = page_id;
    for depth in 0..=MAX_PDF_GRAPH_DEPTH {
        if !seen.insert(current) {
            return Err(import_error(
                Some(page),
                None,
                format!(
                    "cyclic page resource graph at object {} {}",
                    current.0, current.1
                ),
            ));
        }
        resolver.charge(page)?;
        let dictionary = document.get_dictionary(current).map_err(|error| {
            import_error(
                Some(page),
                None,
                format!("page resource node failed: {error}"),
            )
        })?;
        if let Ok(resources) = dictionary.get(b"Resources") {
            return strict_resource_dictionary(document, resources, page, "Resources", resolver)
                .map(Some);
        }
        match dictionary.get(b"Parent") {
            Ok(Object::Reference(parent)) => current = *parent,
            Ok(_) => {
                return Err(import_error(
                    Some(page),
                    None,
                    "page Parent must be an indirect reference",
                ));
            }
            Err(_) => return Ok(None),
        }
        if depth == MAX_PDF_GRAPH_DEPTH {
            return Err(import_error(
                Some(page),
                None,
                format!(
                    "page resource graph exceeds depth limit {}",
                    MAX_PDF_GRAPH_DEPTH
                ),
            ));
        }
    }
    unreachable!("bounded resource traversal returns inside the loop")
}

fn font_entries(
    document: &Document,
    resources: &Dictionary,
    page: u32,
    resolver: &mut ResourceReferenceResolver,
) -> Result<BTreeMap<Vec<u8>, ObjectId>> {
    let Ok(value) = resources.get(b"Font") else {
        return Ok(BTreeMap::new());
    };
    let table = strict_resource_dictionary(document, value, page, "Font table", resolver)?;
    resolver.charge_many(page, table.len())?;
    let mut fonts = BTreeMap::new();
    for (name, value) in table.iter() {
        let id = value.as_reference().map_err(|_| {
            import_error(
                Some(page),
                None,
                format!(
                    "Font /{} member must be an indirect reference",
                    String::from_utf8_lossy(name)
                ),
            )
        })?;
        let label = format!("Font /{}", String::from_utf8_lossy(name));
        let (id, object) = strict_referenced_object(document, id, page, &label, resolver)?;
        let dictionary = object.as_dict().map_err(|_| {
            import_error(
                Some(page),
                None,
                format!("{label} must reference a dictionary"),
            )
        })?;
        if dictionary.get(b"Type").and_then(Object::as_name).ok() != Some(b"Font") {
            return Err(import_error(
                Some(page),
                None,
                format!("{label} dictionary must declare /Type /Font"),
            ));
        }
        dictionary
            .get(b"Subtype")
            .and_then(Object::as_name)
            .map_err(|_| {
                import_error(
                    Some(page),
                    None,
                    format!("{label} dictionary must declare a name Subtype"),
                )
            })?;
        fonts.insert(name.clone(), id);
    }
    Ok(fonts)
}

fn strict_resource_dictionary<'a>(
    document: &'a Document,
    mut value: &'a Object,
    page: u32,
    label: &str,
    resolver: &mut ResourceReferenceResolver,
) -> Result<&'a lopdf::Dictionary> {
    if let Object::Reference(id) = value {
        value = resolver.resolve(document, *id, page, label)?.1;
    }
    value.as_dict().map_err(|_| {
        import_error(
            Some(page),
            None,
            format!("{label} must resolve to a dictionary"),
        )
    })
}

fn strict_referenced_object<'a>(
    document: &'a Document,
    id: ObjectId,
    page: u32,
    label: &str,
    resolver: &mut ResourceReferenceResolver,
) -> Result<(ObjectId, &'a Object)> {
    resolver.resolve(document, id, page, label)
}

fn xobject_entries(
    document: &Document,
    resources: &lopdf::Dictionary,
    page: u32,
    resolver: &mut ResourceReferenceResolver,
) -> Result<Vec<(Vec<u8>, ObjectId)>> {
    let Ok(value) = resources.get(b"XObject") else {
        return Ok(Vec::new());
    };
    let table = strict_resource_dictionary(document, value, page, "XObject table", resolver)?;
    resolver.charge_many(page, table.len())?;
    let mut entries = Vec::with_capacity(table.len());
    for (name, value) in table.iter() {
        let id = value.as_reference().map_err(|_| {
            import_error(
                Some(page),
                None,
                format!(
                    "XObject /{} member must be an indirect reference",
                    String::from_utf8_lossy(name)
                ),
            )
        })?;
        let label = format!("XObject /{}", String::from_utf8_lossy(name));
        let (id, object) = strict_referenced_object(document, id, page, &label, resolver)?;
        let stream = object.as_stream().map_err(|_| {
            import_error(Some(page), None, format!("{label} must reference a stream"))
        })?;
        if stream.dict.get(b"Type").and_then(Object::as_name).ok() != Some(b"XObject") {
            return Err(import_error(
                Some(page),
                None,
                format!("{label} stream must declare /Type /XObject"),
            ));
        }
        let subtype = stream
            .dict
            .get(b"Subtype")
            .and_then(Object::as_name)
            .map_err(|_| {
                import_error(
                    Some(page),
                    None,
                    format!("{label} stream must declare a name Subtype"),
                )
            })?;
        let _ = subtype;
        entries.push((name.clone(), id));
    }
    Ok(entries)
}

fn numbers(operation: &Operation, count: usize, page: u32, offset: usize) -> Result<Vec<f64>> {
    if operation.operands.len() != count {
        return Err(import_error(
            Some(page),
            Some(offset),
            format!("{} requires {count} operands", operation.operator),
        ));
    }
    operation
        .operands
        .iter()
        .take(count)
        .map(|value| {
            object_number(value).map_err(|message| import_error(Some(page), Some(offset), message))
        })
        .collect()
}

fn validate_operator_arity(operation: &Operation, page: u32, offset: usize) -> Result<()> {
    let expected = match operation.operator.as_str() {
        "q" | "Q" | "h" | "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" | "n" | "BT"
        | "ET" | "T*" | "W" | "W*" => Some(0),
        "m" | "l" | "Td" | "TD" | "d" | "Tf" => Some(2),
        "c" | "cm" | "Tm" => Some(6),
        "v" | "y" | "re" => Some(4),
        "rg" | "RG" | "\"" => Some(3),
        "w" | "J" | "j" | "g" | "G" | "Tc" | "Tw" | "Tz" | "TL" | "Ts" | "Tr" | "Tj" | "TJ"
        | "'" | "Do" | "gs" | "sh" | "CS" | "cs" => Some(1),
        _ => None,
    };
    if let Some(expected) = expected
        && operation.operands.len() != expected
    {
        return Err(import_error(
            Some(page),
            Some(offset),
            format!(
                "{} requires exactly {expected} operands, got {}",
                operation.operator,
                operation.operands.len()
            ),
        ));
    }
    Ok(())
}

fn require_current_point(
    current: Option<Point>,
    page: u32,
    offset: usize,
    operator: &str,
) -> Result<Point> {
    current.ok_or_else(|| {
        import_error(
            Some(page),
            Some(offset),
            format!("{operator} has no current point"),
        )
    })
}

fn checked_finite(value: f64, page: u32, offset: usize, label: &str) -> Result<f64> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(import_error(
            Some(page),
            Some(offset),
            format!("{label} produced non-finite geometry"),
        ))
    }
}

fn checked_sum(left: f64, right: f64, page: u32, offset: usize, label: &str) -> Result<f64> {
    checked_finite(left + right, page, offset, label)
}

fn checked_product(left: f64, right: f64, page: u32, offset: usize, label: &str) -> Result<f64> {
    checked_finite(left * right, page, offset, label)
}

fn checked_matrix(matrix: Matrix, page: u32, offset: usize, label: &str) -> Result<Matrix> {
    if [matrix.a, matrix.b, matrix.c, matrix.d, matrix.e, matrix.f]
        .into_iter()
        .all(f64::is_finite)
    {
        Ok(matrix)
    } else {
        Err(import_error(
            Some(page),
            Some(offset),
            format!("{label} produced non-finite geometry"),
        ))
    }
}

fn checked_point(point: Point, page: u32, offset: usize, label: &str) -> Result<Point> {
    if point.x.is_finite() && point.y.is_finite() {
        Ok(point)
    } else {
        Err(import_error(
            Some(page),
            Some(offset),
            format!("{label} produced a non-finite point"),
        ))
    }
}

fn checked_rect(rect: Rect, page: u32, offset: usize, label: &str) -> Result<Rect> {
    if [rect.x, rect.y, rect.width, rect.height]
        .into_iter()
        .all(f64::is_finite)
    {
        Ok(rect)
    } else {
        Err(import_error(
            Some(page),
            Some(offset),
            format!("{label} produced non-finite geometry"),
        ))
    }
}

fn array_numbers(values: &[Object], count: usize, page: u32, offset: usize) -> Result<Vec<f64>> {
    if values.len() != count {
        return Err(import_error(
            Some(page),
            Some(offset),
            format!("array requires exactly {count} values"),
        ));
    }
    values
        .iter()
        .map(|value| {
            object_number(value).map_err(|message| import_error(Some(page), Some(offset), message))
        })
        .collect()
}

fn object_number(value: &Object) -> std::result::Result<f64, String> {
    let number = match value {
        Object::Integer(value) => *value as f64,
        Object::Real(value) => f64::from(*value),
        _ => return Err("expected finite PDF number".to_owned()),
    };
    if !number.is_finite() {
        return Err("non-finite PDF number".to_owned());
    }
    Ok(number)
}

fn number_at(operation: &Operation, index: usize, page: u32, offset: usize) -> Result<f64> {
    operation
        .operands
        .get(index)
        .ok_or_else(|| {
            import_error(
                Some(page),
                Some(offset),
                format!("{} is missing operand {index}", operation.operator),
            )
        })
        .and_then(|value| {
            object_number(value).map_err(|message| import_error(Some(page), Some(offset), message))
        })
}

fn integer_at(operation: &Operation, index: usize, page: u32, offset: usize) -> Result<i64> {
    operation
        .operands
        .get(index)
        .and_then(|value| value.as_i64().ok())
        .ok_or_else(|| {
            import_error(
                Some(page),
                Some(offset),
                format!("{} requires integer operand {index}", operation.operator),
            )
        })
}

fn name_at(operation: &Operation, index: usize, page: u32, offset: usize) -> Result<&[u8]> {
    operation
        .operands
        .get(index)
        .and_then(|value| value.as_name().ok())
        .ok_or_else(|| {
            import_error(
                Some(page),
                Some(offset),
                format!("{} requires name operand {index}", operation.operator),
            )
        })
}

fn string_bytes_at(
    operation: &Operation,
    index: usize,
    page: u32,
    offset: usize,
) -> Result<Vec<u8>> {
    operation
        .operands
        .get(index)
        .and_then(|value| value.as_str().ok())
        .map(Vec::from)
        .ok_or_else(|| {
            import_error(
                Some(page),
                Some(offset),
                format!("{} requires string operand {index}", operation.operator),
            )
        })
}

fn decode_pdf_string(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xfe, 0xff]) {
        String::from_utf16_lossy(
            &bytes[2..]
                .chunks_exact(2)
                .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
                .collect::<Vec<_>>(),
        )
    } else {
        bytes.iter().map(|byte| char::from(*byte)).collect()
    }
}

fn decode_pdf_uri_string(bytes: &[u8]) -> Option<String> {
    let (payload, little_endian) = if let Some(payload) = bytes.strip_prefix(&[0xfe, 0xff]) {
        (payload, false)
    } else if let Some(payload) = bytes.strip_prefix(&[0xff, 0xfe]) {
        (payload, true)
    } else {
        return Some(bytes.iter().map(|byte| char::from(*byte)).collect());
    };
    if payload.len() % 2 != 0 {
        return None;
    }
    char::decode_utf16(payload.chunks_exact(2).map(|chunk| {
        if little_endian {
            u16::from_le_bytes([chunk[0], chunk[1]])
        } else {
            u16::from_be_bytes([chunk[0], chunk[1]])
        }
    }))
    .collect::<std::result::Result<String, _>>()
    .ok()
}

fn transformed_point(
    operation: &Operation,
    state: &GraphicsState,
    geometry: PageGeometry,
    page: u32,
    offset: usize,
    start: usize,
) -> Result<Point> {
    let x = number_at(operation, start, page, offset)?;
    let y = number_at(operation, start + 1, page, offset)?;
    checked_point(
        geometry.normalize(state.ctm.point(x, y)),
        page,
        offset,
        "transformed path",
    )
}

fn point_distance(first: Point, second: Point) -> f64 {
    (second.x - first.x).hypot(second.y - first.y)
}

fn advance_text_matrix(
    state: &mut GraphicsState,
    advance: f64,
    page: u32,
    offset: usize,
) -> Result<()> {
    state.text_matrix = checked_matrix(
        state.text_matrix.translated(advance, 0.0),
        page,
        offset,
        "text advance",
    )?;
    Ok(())
}

fn finite_positive(value: f64, page: u32, offset: usize, label: &str) -> Result<f64> {
    if !value.is_finite() || value <= 0.0 {
        Err(import_error(
            (page != 0).then_some(page),
            (page != 0).then_some(offset),
            format!("{label} must be finite and positive"),
        ))
    } else {
        Ok(value)
    }
}

fn finite_nonnegative(value: f64, page: u32, offset: usize, label: &str) -> Result<f64> {
    if !value.is_finite() || value < 0.0 {
        Err(import_error(
            (page != 0).then_some(page),
            (page != 0).then_some(offset),
            format!("{label} must be finite and nonnegative"),
        ))
    } else {
        Ok(value)
    }
}

fn gray(value: f64, page: u32, offset: usize) -> Result<Color> {
    component(value, page, offset).map(|value| Color {
        r: value,
        g: value,
        b: value,
        a: 1.0,
    })
}
fn rgb(operation: &Operation, page: u32, offset: usize) -> Result<Color> {
    let v = numbers(operation, 3, page, offset)?;
    Ok(Color {
        r: component(v[0], page, offset)?,
        g: component(v[1], page, offset)?,
        b: component(v[2], page, offset)?,
        a: 1.0,
    })
}
fn component(value: f64, page: u32, offset: usize) -> Result<f64> {
    if (0.0..=1.0).contains(&value) {
        Ok(value)
    } else {
        Err(import_error(
            Some(page),
            Some(offset),
            "colour component must be between zero and one",
        ))
    }
}

fn parse_dash(operation: &Operation, page: u32, offset: usize) -> Result<Option<DashState>> {
    let values = operation
        .operands
        .first()
        .and_then(|value| value.as_array().ok())
        .ok_or_else(|| import_error(Some(page), Some(offset), "dash operator requires an array"))?;
    let mut dash = Vec::with_capacity(values.len());
    for value in values {
        dash.push(finite_nonnegative(
            object_number(value)
                .map_err(|message| import_error(Some(page), Some(offset), message))?,
            page,
            offset,
            "dash length",
        )?);
    }
    let phase = number_at(operation, 1, page, offset)?;
    if !phase.is_finite() || phase < 0.0 {
        return Err(import_error(
            Some(page),
            Some(offset),
            "dash phase must be finite and nonnegative",
        ));
    }
    if dash.is_empty() {
        if phase != 0.0 {
            return Err(import_error(
                Some(page),
                Some(offset),
                "an empty dash array requires zero phase",
            ));
        }
        Ok(None)
    } else if dash.iter().all(|value| *value == 0.0) {
        Err(import_error(
            Some(page),
            Some(offset),
            "dash array must contain at least one nonzero element",
        ))
    } else {
        Ok(Some(DashState {
            pattern: dash,
            phase,
        }))
    }
}

fn normalized_dash(pattern: &[f64], phase: f64) -> Option<Vec<f64>> {
    let mut cycle = pattern.to_vec();
    if cycle.len() % 2 == 1 {
        cycle.extend_from_slice(pattern);
    }
    let total = cycle.iter().sum::<f64>();
    if phase == 0.0 {
        return Some(cycle);
    }
    let mut remaining_phase = phase % total;
    if remaining_phase == 0.0 {
        return Some(cycle);
    }
    let mut index = 0usize;
    while remaining_phase > cycle[index] {
        remaining_phase -= cycle[index];
        index = (index + 1) % cycle.len();
    }
    if remaining_phase != cycle[index] {
        return None;
    }
    index = (index + 1) % cycle.len();
    if index % 2 == 1 {
        return None;
    }
    cycle.rotate_left(index);
    Some(cycle)
}

fn require_text(state: &GraphicsState, page: u32, offset: usize) -> Result<()> {
    if state.in_text {
        Ok(())
    } else {
        Err(import_error(
            Some(page),
            Some(offset),
            "text operator outside BT/ET",
        ))
    }
}

fn positive_i64(dictionary: &Dictionary, key: &[u8], page: u32, offset: usize) -> Result<i64> {
    let value = dictionary
        .get(key)
        .and_then(Object::as_i64)
        .map_err(|error| {
            import_error(
                Some(page),
                Some(offset),
                format!("image {} failed: {error}", String::from_utf8_lossy(key)),
            )
        })?;
    if value <= 0 {
        Err(import_error(
            Some(page),
            Some(offset),
            "image dimension must be positive",
        ))
    } else {
        Ok(value)
    }
}

fn filter_names(dictionary: &Dictionary) -> std::result::Result<Vec<String>, &'static str> {
    match dictionary.get(b"Filter") {
        Ok(Object::Name(name)) => Ok(vec![String::from_utf8_lossy(name).into_owned()]),
        Ok(Object::Array(values)) => values
            .iter()
            .map(|value| {
                value
                    .as_name()
                    .map(|name| String::from_utf8_lossy(name).into_owned())
                    .map_err(|_| "image Filter array members must be names")
            })
            .collect(),
        Err(_) => Ok(Vec::new()),
        Ok(_) => Err("image Filter must be a name or array of names"),
    }
}

fn unsupported_image_semantics(
    dictionary: &Dictionary,
    filters: &[String],
) -> std::result::Result<Option<String>, String> {
    for key in [b"Mask".as_slice(), b"SMask", b"Decode"] {
        if dictionary.has(key) {
            return Ok(Some(format!(
                "image {} semantics are unsupported",
                String::from_utf8_lossy(key)
            )));
        }
    }
    let image_mask = match dictionary.get(b"ImageMask") {
        Ok(Object::Boolean(value)) => *value,
        Ok(_) => return Err("image ImageMask must be a boolean".to_owned()),
        Err(_) => false,
    };
    if image_mask {
        return Ok(Some("image masks are unsupported".to_owned()));
    }
    if filters.is_empty() || filters == ["FlateDecode"] || filters == ["DCTDecode"] {
        let bits = image_bits(dictionary)?;
        if bits != 8 {
            return Ok(Some(format!("image bit depth {bits} is unsupported")));
        }
        let space = dictionary
            .get(b"ColorSpace")
            .and_then(Object::as_name)
            .unwrap_or_default();
        if !matches!(space, b"DeviceGray" | b"DeviceRGB") {
            return Ok(Some(format!(
                "image colour space {} is unsupported",
                String::from_utf8_lossy(space)
            )));
        }
    }
    Ok(None)
}

fn image_bits(dictionary: &Dictionary) -> std::result::Result<i64, String> {
    match dictionary.get(b"BitsPerComponent") {
        Ok(Object::Integer(value)) => Ok(*value),
        Ok(_) => Err("image BitsPerComponent must be an integer".to_owned()),
        Err(_) => Ok(8),
    }
}

fn jpeg_dimensions(data: &[u8]) -> std::result::Result<(u32, u32), String> {
    if !data.starts_with(&[0xff, 0xd8]) {
        return Err("DCT image is missing the JPEG SOI marker".to_owned());
    }
    let mut cursor = 2usize;
    while cursor < data.len() {
        while data.get(cursor) == Some(&0xff) {
            cursor += 1;
        }
        let marker = *data
            .get(cursor)
            .ok_or_else(|| "DCT image has a truncated JPEG marker".to_owned())?;
        cursor += 1;
        if marker == 0xd9 || marker == 0xda {
            break;
        }
        if marker == 0x01 || (0xd0..=0xd8).contains(&marker) {
            continue;
        }
        let length = data
            .get(cursor..cursor + 2)
            .and_then(|bytes| <[u8; 2]>::try_from(bytes).ok())
            .map(u16::from_be_bytes)
            .map(usize::from)
            .ok_or_else(|| "DCT image has a truncated JPEG segment length".to_owned())?;
        if length < 2 {
            return Err("DCT image has an invalid JPEG segment length".to_owned());
        }
        let end = cursor
            .checked_add(length)
            .ok_or_else(|| "DCT image JPEG segment length overflow".to_owned())?;
        if end > data.len() {
            return Err("DCT image has a truncated JPEG segment".to_owned());
        }
        if matches!(
            marker,
            0xc0 | 0xc1
                | 0xc2
                | 0xc3
                | 0xc5
                | 0xc6
                | 0xc7
                | 0xc9
                | 0xca
                | 0xcb
                | 0xcd
                | 0xce
                | 0xcf
        ) {
            if length < 8 {
                return Err("DCT image has a truncated JPEG SOF segment".to_owned());
            }
            let height = u16::from_be_bytes([data[cursor + 3], data[cursor + 4]]);
            let width = u16::from_be_bytes([data[cursor + 5], data[cursor + 6]]);
            if width == 0 || height == 0 {
                return Err("DCT image has zero intrinsic JPEG dimensions".to_owned());
            }
            return Ok((u32::from(width), u32::from(height)));
        }
        cursor = end;
    }
    Err("DCT image has no supported JPEG SOF marker".to_owned())
}

fn decode_png(
    stream: &Stream,
    width: u32,
    height: u32,
    limit: usize,
) -> std::result::Result<(Vec<u8>, usize), String> {
    let expected = decoded_image_len(&stream.dict, width, height)?;
    if expected > limit {
        return Err(format!(
            "decoded image bytes {expected} exceed limit {limit}"
        ));
    }
    let data = stream
        .decompressed_content_with_limit(limit)
        .map_err(|error| format!("image decompression failed: {error}"))?;
    if data.len() != expected {
        return Err(format!(
            "decoded image length {} does not match expected {expected}",
            data.len()
        ));
    }
    let channels = match stream
        .dict
        .get(b"ColorSpace")
        .and_then(Object::as_name)
        .unwrap_or_default()
    {
        b"DeviceGray" => 1,
        _ => 3,
    };
    let mut rgba = Vec::with_capacity(expected / channels * 4);
    for pixel in data.chunks_exact(channels) {
        let (r, g, b) = if channels == 1 {
            (pixel[0], pixel[0], pixel[0])
        } else {
            (pixel[0], pixel[1], pixel[2])
        };
        rgba.extend_from_slice(&[r, g, b, 255]);
    }
    let size = tiny_skia::IntSize::from_wh(width, height)
        .ok_or_else(|| "invalid image dimensions".to_owned())?;
    let png = tiny_skia::Pixmap::from_vec(rgba, size)
        .ok_or_else(|| "invalid image buffer".to_owned())?
        .encode_png()
        .map_err(|error| format!("PNG encode failed: {error}"))?;
    Ok((png, data.len()))
}

fn decoded_image_len(
    dictionary: &Dictionary,
    width: u32,
    height: u32,
) -> std::result::Result<usize, String> {
    if image_bits(dictionary)? != 8 {
        return Err("only 8-bit Flate images are supported".to_owned());
    }
    let space = dictionary
        .get(b"ColorSpace")
        .and_then(Object::as_name)
        .map_err(|_| "image colour space is missing or unsupported".to_owned())?;
    let channels = match space {
        b"DeviceGray" => 1usize,
        b"DeviceRGB" => 3usize,
        _ => {
            return Err(format!(
                "image colour space {} is unsupported",
                String::from_utf8_lossy(space)
            ));
        }
    };
    usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(channels))
        .ok_or_else(|| "image byte count overflow".to_owned())
}

fn image_corners(
    geometry: PageGeometry,
    matrix: Matrix,
    page: u32,
    offset: usize,
) -> Result<[Point; 4]> {
    let mut corners = [
        geometry.normalize(matrix.point(0.0, 0.0)),
        geometry.normalize(matrix.point(1.0, 0.0)),
        geometry.normalize(matrix.point(0.0, 1.0)),
        geometry.normalize(matrix.point(1.0, 1.0)),
    ];
    for corner in &mut corners {
        *corner = checked_point(*corner, page, offset, "image corner")?;
    }
    Ok(corners)
}

fn points_bounds(points: &[Point; 4]) -> Rect {
    let min_x = points
        .iter()
        .map(|point| point.x)
        .fold(f64::INFINITY, f64::min);
    let max_x = points
        .iter()
        .map(|point| point.x)
        .fold(f64::NEG_INFINITY, f64::max);
    let min_y = points
        .iter()
        .map(|point| point.y)
        .fold(f64::INFINITY, f64::min);
    let max_y = points
        .iter()
        .map(|point| point.y)
        .fold(f64::NEG_INFINITY, f64::max);
    Rect {
        x: min_x,
        y: min_y,
        width: max_x - min_x,
        height: max_y - min_y,
    }
}

fn strict_uri_action_dictionary(
    dictionary: &Dictionary,
    page: Option<u32>,
    path: &str,
) -> Result<String> {
    let kind = dictionary
        .get(b"S")
        .and_then(Object::as_name)
        .map_err(|_| {
            import_error(
                page,
                None,
                format!("action at {path} must declare a name S"),
            )
        })?;
    if kind != b"URI" {
        return Err(import_error(
            page,
            None,
            format!(
                "non-URI action {} is unsupported at {path}",
                String::from_utf8_lossy(kind)
            ),
        ));
    }
    let uri = dictionary
        .get(b"URI")
        .and_then(Object::as_str)
        .map_err(|_| import_error(page, None, format!("URI action at {path} is malformed")))
        .and_then(|bytes| {
            decode_pdf_uri_string(bytes).ok_or_else(|| {
                import_error(page, None, format!("URI action at {path} is malformed"))
            })
        })?;
    if !safe_uri(&uri) {
        return Err(import_error(
            page,
            None,
            format!("URI action at {path} has an unsupported or malformed URI"),
        ));
    }
    Ok(uri)
}

fn safe_uri(uri: &str) -> bool {
    if uri.is_empty()
        || uri.trim() != uri
        || uri
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return false;
    }
    let lower = uri.to_ascii_lowercase();
    ["https://", "http://", "mailto:"]
        .into_iter()
        .any(|prefix| {
            lower
                .strip_prefix(prefix)
                .is_some_and(|value| !value.is_empty())
        })
}

fn sanitize_content_comments(data: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(data.len());
    let mut literal_depth = 0usize;
    let mut escaped = false;
    let mut hex_string = false;
    let mut index = 0usize;
    while index < data.len() {
        let byte = data[index];
        if literal_depth > 0 {
            output.push(byte);
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'(' {
                literal_depth += 1;
            } else if byte == b')' {
                literal_depth -= 1;
            }
            index += 1;
            continue;
        }
        if hex_string {
            output.push(byte);
            if byte == b'>' {
                hex_string = false;
            }
            index += 1;
            continue;
        }
        match byte {
            b'(' => {
                literal_depth = 1;
                output.push(byte);
                index += 1;
            }
            b'<' if data.get(index + 1) != Some(&b'<') => {
                hex_string = true;
                output.push(byte);
                index += 1;
            }
            b'%' => {
                while index < data.len() && !matches!(data[index], b'\r' | b'\n') {
                    index += 1;
                }
                output.push(b'\n');
                if data.get(index) == Some(&b'\r') {
                    index += 1;
                }
                if data.get(index) == Some(&b'\n') {
                    index += 1;
                }
            }
            _ => {
                output.push(byte);
                index += 1;
            }
        }
    }
    output
}

fn raster_pixels(width: f64, height: f64, dpi: f64) -> Result<u64> {
    finite_positive(dpi, 0, 0, "DPI")?;
    let width = (width * dpi / 72.0).ceil();
    let height = (height * dpi / 72.0).ceil();
    if width > u32::MAX as f64 || height > u32::MAX as f64 {
        return Err(import_error(None, None, "raster dimensions overflow"));
    }
    (width as u64)
        .checked_mul(height as u64)
        .ok_or_else(|| import_error(None, None, "raster pixel count overflow"))
}

fn points_to_emu(points: f64) -> Result<Emu> {
    if !points.is_finite() {
        return Err(import_error(None, None, "non-finite point coordinate"));
    }
    let value = points * EMU_PER_POINT;
    if value < i64::MIN as f64 || value > i64::MAX as f64 {
        return Err(import_error(
            None,
            None,
            "point coordinate exceeds EMU range",
        ));
    }
    Ok(Emu(value.trunc() as i64))
}

fn rect_emu(rect: Rect) -> Result<(Emu, Emu, Emu, Emu)> {
    Ok((
        points_to_emu(rect.x)?,
        points_to_emu(rect.y)?,
        points_to_emu(rect.width.max(1.0 / EMU_PER_POINT))?,
        points_to_emu(rect.height.max(1.0 / EMU_PER_POINT))?,
    ))
}

fn solid_fill(color: Color) -> Result<Fill> {
    let xml = format!(
        r#"<a:solidFill xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:srgbClr val="{}"/></a:solidFill>"#,
        color_hex(color)
    );
    Fill::from_xml(xml.as_bytes()).map_err(|error| {
        import_error(
            None,
            None,
            format!("solid fill construction failed: {error}"),
        )
    })
}

fn color_hex(color: Color) -> String {
    format!(
        "{:02X}{:02X}{:02X}",
        (color.r.clamp(0.0, 1.0) * 255.0).round() as u8,
        (color.g.clamp(0.0, 1.0) * 255.0).round() as u8,
        (color.b.clamp(0.0, 1.0) * 255.0).round() as u8
    )
}

fn append_custom_path(
    presentation: &mut Presentation,
    slide_index: usize,
    element: &PathElement,
) -> Result<()> {
    let path_bounds = element
        .path
        .bounds()
        .ok_or_else(|| import_error(None, None, "painted path has no bounds"))?;
    let stroke_margin = element
        .stroke
        .as_ref()
        .map_or(0.0, |stroke| stroke.width / 2.0);
    let bounds = Rect {
        x: path_bounds.x - stroke_margin,
        y: path_bounds.y - stroke_margin,
        width: path_bounds.width + stroke_margin * 2.0,
        height: path_bounds.height + stroke_margin * 2.0,
    };
    let (left, top, width, height) = rect_emu(bounds)?;
    let mut commands = String::new();
    for command in &element.path.commands {
        match command {
            PathCommand::MoveTo(point) => commands.push_str(&format!(r#"<a:moveTo><a:pt x="{}" y="{}"/></a:moveTo>"#, relative_coord(point.x, bounds.x, bounds.width), relative_coord(point.y, bounds.y, bounds.height))),
            PathCommand::LineTo(point) => commands.push_str(&format!(r#"<a:lnTo><a:pt x="{}" y="{}"/></a:lnTo>"#, relative_coord(point.x, bounds.x, bounds.width), relative_coord(point.y, bounds.y, bounds.height))),
            PathCommand::CurveTo { c1, c2, to } => commands.push_str(&format!(r#"<a:cubicBezTo><a:pt x="{}" y="{}"/><a:pt x="{}" y="{}"/><a:pt x="{}" y="{}"/></a:cubicBezTo>"#, relative_coord(c1.x, bounds.x, bounds.width), relative_coord(c1.y, bounds.y, bounds.height), relative_coord(c2.x, bounds.x, bounds.width), relative_coord(c2.y, bounds.y, bounds.height), relative_coord(to.x, bounds.x, bounds.width), relative_coord(to.y, bounds.y, bounds.height))),
            PathCommand::Close => commands.push_str("<a:close/>"),
        }
    }
    let geometry_xml = format!(
        r#"<a:custGeom xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:avLst/><a:gdLst/><a:ahLst/><a:cxnLst/><a:rect l="l" t="t" r="r" b="b"/><a:pathLst><a:path w="100000" h="100000" fill="{}" stroke="{}">{commands}</a:path></a:pathLst></a:custGeom>"#,
        if element.fill.is_some() {
            "norm"
        } else {
            "none"
        },
        element.stroke.is_some()
    );
    let geometry = CT_CustomGeometry2D::from_xml(geometry_xml.as_bytes()).map_err(|error| {
        import_error(
            None,
            None,
            format!("custom path construction failed: {error}"),
        )
    })?;
    let slide_count = presentation.slides.len();
    let slide = presentation
        .slides
        .get_mut(slide_index)
        .ok_or(Error::UnknownSlideIndex {
            index: slide_index,
            slide_count,
        })?;
    let tree = &mut Arc::make_mut(slide).slide.common_slide_data.shape_tree;
    let id = ShapeIdAllocator::scan(tree).allocate();
    let mut shape = CT_Shape::new_preset(
        id,
        &format!("PDF path {id}"),
        "rect",
        positioned_transform(left, top, width, height),
    )
    .map_err(|error| import_error(None, None, format!("path shape failed: {error}")))?;
    shape.shape_properties.preset_geometry = None;
    shape.shape_properties.custom_geometry = Some(geometry);
    shape.shape_properties.fill = element.fill.as_ref().and_then(|paint| match paint {
        Paint::Solid(color) => solid_fill(*color).ok(),
        _ => None,
    });
    shape.shape_properties.line = element.stroke.as_ref().map(stroke_line).transpose()?;
    tree.append_child(ShapeTreeChild::Shape(shape));
    Ok(())
}

fn stroke_line(stroke: &Stroke) -> Result<CT_LineProperties> {
    let color = match &stroke.paint {
        Paint::Solid(color) => *color,
        _ => Color::BLACK,
    };
    let cap = match stroke.cap {
        LineCap::Butt => "flat",
        LineCap::Round => "rnd",
        LineCap::Square => "sq",
    };
    let join = match stroke.join {
        LineJoin::Miter => "<a:miter/>",
        LineJoin::Round => "<a:round/>",
        LineJoin::Bevel => "<a:bevel/>",
    };
    let dash = if let Some(values) = &stroke.dash {
        let mut values = values.clone();
        if values.len() % 2 == 1 {
            let repeated = values.clone();
            values.extend(repeated);
        }
        let width = stroke.width.max(1.0 / EMU_PER_POINT);
        let mut xml = String::from("<a:custDash>");
        for pair in values.chunks_exact(2) {
            let dash = drawingml_dash_units(pair[0], width).ok_or_else(|| {
                import_error(
                    None,
                    None,
                    "dash member cannot produce a positive DrawingML dash stop",
                )
            })?;
            let space = drawingml_dash_units(pair[1], width).ok_or_else(|| {
                import_error(
                    None,
                    None,
                    "dash member cannot produce a positive DrawingML dash stop",
                )
            })?;
            xml.push_str(&format!(r#"<a:ds d="{dash}" sp="{space}"/>"#));
        }
        xml.push_str("</a:custDash>");
        xml
    } else {
        String::new()
    };
    let xml = format!(
        r#"<a:ln xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" w="{}" cap="{cap}"><a:solidFill><a:srgbClr val="{}"/></a:solidFill>{dash}{join}</a:ln>"#,
        (stroke.width * EMU_PER_POINT).trunc() as i64,
        color_hex(color)
    );
    CT_LineProperties::from_xml(xml.as_bytes()).map_err(|error| {
        import_error(
            None,
            None,
            format!("path line construction failed: {error}"),
        )
    })
}

fn drawingml_dash_units(value: f64, width: f64) -> Option<i32> {
    let units = value / width.max(1.0 / EMU_PER_POINT) * 100_000.0;
    (units.is_finite() && units >= 1.0).then(|| units.min(i32::MAX as f64).trunc() as i32)
}

fn relative_coord(value: f64, origin: f64, span: f64) -> i64 {
    if span.abs() < f64::EPSILON {
        0
    } else {
        ((value - origin) * 100_000.0 / span).trunc() as i64
    }
}

fn append_link_overlay(
    presentation: &mut Presentation,
    slide_index: usize,
    rect: Rect,
    uri: &str,
) -> Result<()> {
    let slide_count = presentation.slides.len();
    let slide = presentation
        .slides
        .get_mut(slide_index)
        .ok_or(Error::UnknownSlideIndex {
            index: slide_index,
            slide_count,
        })?;
    let relationships = presentation
        .package
        .part_rels
        .entry(slide.part_name.clone())
        .or_default();
    let relationship_id = relationships.add_external(rel_types::HYPERLINK, uri);
    let (left, top, width, height) = rect_emu(rect)?;
    let tree = &mut Arc::make_mut(slide).slide.common_slide_data.shape_tree;
    let id = ShapeIdAllocator::scan(tree).allocate();
    let xml = format!(
        r#"<p:sp xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:nvSpPr><p:cNvPr id="{id}" name="PDF link {id}"><a:hlinkClick r:id="{relationship_id}"/></p:cNvPr><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="{}" y="{}"/><a:ext cx="{}" cy="{}"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/><a:ln><a:noFill/></a:ln></p:spPr></p:sp>"#,
        left.0, top.0, width.0, height.0
    );
    let shape = CT_Shape::from_xml(xml.as_bytes()).map_err(|error| {
        import_error(
            None,
            None,
            format!("link overlay construction failed: {error}"),
        )
    })?;
    tree.append_child(ShapeTreeChild::Shape(shape));
    Ok(())
}

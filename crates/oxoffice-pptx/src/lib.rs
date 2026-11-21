//! Public facade for PresentationML packages.
//!
//! The `default-template` feature embeds a 16:9 zero-slide template derived
//! from the MIT-licensed python-pptx 1.0.2 default template. The slide size was
//! changed to 12,192,000 by 6,858,000 EMU, the size kind was changed to
//! `screen16x9`. Master and layout placeholder rectangles match the retained
//! Office 16:9 authority. Fresh core dates are fixed to 2000-01-01T00:00:00Z,
//! with no inherited author, last modifier or description. Imported packages
//! keep their metadata. python-pptx generated the notes-master infrastructure.

mod embedded;

pub use embedded::{
    EmbeddedContentInfo, EmbeddedContentKind, EmbeddedMutationPolicy, EmbeddedSignatureState,
};

use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(feature = "render")]
use diagram::{DiagramResources, ScopedDiagramResources};
use oxoffice_chart::CT_ChartSpace;
pub use oxoffice_chart::{ChartData, ChartKind, LegendPosition, LegendSettings, RgbColor};
use oxoffice_core::OxmlError;
pub use oxoffice_core::app_properties::AppProperties;
pub use oxoffice_core::core_properties::CoreProperties;
pub use oxoffice_core::custom_properties::{CustomProperties, CustomProperty, CustomPropertyValue};
pub use oxoffice_core::units::{Angle, Emu, Percent1000};
use oxoffice_core::xml::escape_attribute_value;
use oxoffice_drawing::color::ColorMap;
pub use oxoffice_drawing::color::{ColorChoice, ColorTransform};
pub use oxoffice_drawing::effect::{CT_EffectList, CT_OuterShadowEffect, RectAlignment};
use oxoffice_drawing::fill::RelativeRect;
pub use oxoffice_drawing::fill::{
    Fill, GradientFill, GradientGeometry, GradientStop, NoFill, PathGradientKind, PatternFill,
    SolidFill,
};
use oxoffice_drawing::geometry::{CT_PresetGeometry2D, Guide, GuideOp, GuideOperand};
pub use oxoffice_drawing::line::{
    CT_LineProperties, LineDash, LineEnd, LineEndSize, LineEndType, PresetDash,
    ST_PresetLineDashVal,
};
use oxoffice_drawing::namespace::A_NS;
use oxoffice_drawing::shape_props::CT_ShapeProperties;
#[cfg(feature = "render")]
use oxoffice_drawing::table::CT_TableStyleList;
use oxoffice_drawing::table::{CT_Table, CT_TableCell, CT_TableCellProperties, CT_TableProperties};
use oxoffice_drawing::text::CT_TextListStyle;
use oxoffice_drawing::text::{
    CT_RegularTextRun, CT_TextBody, CT_TextField, CT_TextParagraph, Coordinate32Value,
    NormalAutofit, TextAutofit, TextRun, TextWrap,
};
pub use oxoffice_drawing::text::{
    CT_TextCharacterProperties, CT_TextParagraphProperties, TextAlignment, TextAnchor,
    TextAutoNumber, TextAutoNumberScheme, TextBullet, TextBulletCharacter, TextBulletChoice,
    TextBulletColor, TextBulletSize, TextBulletSizeValue, TextFont, TextNoBullet, TextPointValue,
    TextSpacing, TextStrike, TextUnderline,
};
use oxoffice_drawing::theme::CT_OfficeStyleSheet;
use oxoffice_drawing::xfrm::{CT_Point2D, CT_PositiveSize2D, CT_Transform2D};
use oxoffice_layout::MediaId;
#[cfg(feature = "render")]
use oxoffice_layout::{
    Color, FontManager, GlyphRun, GroupElement, LayoutResult, PageFrame, Path as LayoutPath,
    PathElement, Point, PositionedElement, Rect, Transform,
};
use oxoffice_media::{
    ImageFormat, MediaNamer, audio_video_signature_matches, is_safe_content_type, probe, resolve,
};
use oxoffice_opc::content_types;
use oxoffice_opc::relationship::{Relationship, rel_types};
#[cfg(feature = "digital-signatures")]
pub use oxoffice_opc::{
    CoveredRelationship, SignatureIssue, SignatureReport, SignerCertificateIdentity,
};
use oxoffice_opc::{OpcError, Relationships, write_atomic_file};
pub use oxoffice_opc::{OpcPackage, PackageReadLimits, SaveOptions};
#[cfg(feature = "render")]
pub use oxoffice_pdf::{PdfConformance, RasterFormat, RasterOptions, RasterOutput};
#[cfg(feature = "render")]
pub use oxoffice_pptx_layout::timeline::{
    EvaluatedFrameState, EvaluatedMediaState, MediaPlaybackPhase, TimelinePosition,
};
#[cfg(feature = "render")]
use oxoffice_pptx_layout::timeline::{ResolvedTimelineSlide, evaluate_media_playback};
#[cfg(feature = "render")]
use oxoffice_pptx_layout::{
    ChartResource, FlattenedItem, FlattenedSource, ResolveCtx, ResolvedAutofit, ResolvedContent,
    ResolvedSlide, ResolvedSlideTextDirections, ScopedChartResources, ScopedHyperlinkTargets,
    ScopedMediaFailures, ScopedMediaIds,
};
pub use oxoffice_pptx_oxml::comments::{Comment, CommentAuthor, CommentReply};
use oxoffice_pptx_oxml::comments::{
    CommentAuthorList, CommentList, append_classic_comment_dependency,
    classic_comment_dependencies, import_classic_comments,
};
use oxoffice_pptx_oxml::connector::CT_ConnectionShape;
pub use oxoffice_pptx_oxml::diagram::{
    CT_DiagramColorsDefinition, CT_DiagramData, CT_DiagramDrawing, CT_DiagramLayoutDefinition,
    CT_DiagramStyleDefinition, DiagramConnection, DiagramConnectionKind, DiagramLayoutFamily,
    DiagramPoint, DiagramPointKind, DiagramRelationshipIds, DiagramShapeStyle,
};
use oxoffice_pptx_oxml::graphic_frame::{CT_GraphicFrame, GraphicDataPayload};
use oxoffice_pptx_oxml::inheritance::inherited_xfrm;
use oxoffice_pptx_oxml::namespace::{P_NS, R_NS};
use oxoffice_pptx_oxml::notes_parts::{CT_HandoutMaster, CT_NotesMaster, CT_NotesSlide};
pub use oxoffice_pptx_oxml::picture::MediaKind;
use oxoffice_pptx_oxml::picture::{CT_Picture, MediaSource as PictureMediaSource, PictureMedia};
use oxoffice_pptx_oxml::placeholder::PlaceholderKey;
use oxoffice_pptx_oxml::placeholder::{CT_Placeholder, PhType};
use oxoffice_pptx_oxml::presentation::{
    CT_EmbeddedFontListEntry, CT_Presentation, CT_SlideId, CT_SlideMasterId,
    custom_show_relationship_ids,
};
pub use oxoffice_pptx_oxml::presentation::{EmbeddedFontKind, Section};
use oxoffice_pptx_oxml::relmap::{
    relationship_ids, release_hyperlinks, rewrite_exact_rel_ids, rewrite_rel_ids,
};
use oxoffice_pptx_oxml::shape_tree::{
    CT_GroupShape, CT_Shape, CT_ShapeTree, ShapeIdAllocator, ShapeTreeChild, rewrite_shape_ids,
    rewrite_shape_ids_mapped,
};
use oxoffice_pptx_oxml::slide_parts::CT_CommonSlideData;
use oxoffice_pptx_oxml::slide_parts::ColorMapOverrideKind;
use oxoffice_pptx_oxml::slide_parts::{
    BackgroundRendering, CT_HeaderFooter, CT_Slide, CT_SlideLayout, CT_SlideMaster,
};
pub use oxoffice_pptx_oxml::timing::MediaPlaybackTrigger;
use oxoffice_pptx_oxml::timing::{CT_Timing, MediaCommandKind, MediaDisplayPolicy};
#[cfg(feature = "render")]
use oxoffice_pptx_oxml::timing::{TimingNode, TimingTarget};
#[cfg(feature = "render")]
use oxoffice_pptx_render::timeline::{
    compose_morph_transition, compose_transition, layout_timeline_slide_with_displayed_number,
};
#[cfg(feature = "render")]
use oxoffice_pptx_render::{
    MediaData, RenderInput, layout_presentation_with_displayed_numbers,
    layout_presentation_with_font_manager_and_text_directions_mut,
};
use quick_xml::events::{BytesStart as XmlBytesStart, Event as XmlEvent};
use quick_xml::name::{Namespace as XmlNamespace, ResolveResult as XmlResolveResult};
use quick_xml::reader::NsReader;
use quick_xml::{Writer as XmlWriter, XmlVersion};
use thiserror::Error;

#[cfg(feature = "render")]
mod animation;
#[cfg(feature = "render")]
mod diagram;
mod html;
#[cfg(feature = "default-template")]
mod odp;
#[cfg(feature = "render")]
mod pdf;
#[cfg(feature = "render")]
pub use animation::{
    AnimationExportOptions, AnimationFormat, AnimationSegment, AnimationTransition,
    DeterministicAnimation, GifLoopBehavior,
};
#[cfg(feature = "default-template")]
pub use html::{HtmlDiagnostic, HtmlImageResource, HtmlReadResult};
pub use html::{PresentationTextExport, PresentationTextExportOptions};
#[cfg(feature = "default-template")]
pub use odp::{OdpDiagnostic, OdpReadResult, OdpWriteResult};
#[cfg(feature = "render")]
pub use pdf::{PdfImportDiagnostic, PdfImportLimits, PdfImportMode, PdfImportResult};

#[cfg(feature = "render")]
mod render;

#[cfg(feature = "render")]
pub use render::{
    AutofitRefreshResult, AutofitUpdate, DeterministicMediaTimelineFrame,
    DeterministicTimelineFrame, FitFrameReport, FitReport, HandoutLayout, MediaFallbackPolicy,
    TextFrameLayout, TextFrameLocation,
};
#[cfg(feature = "render")]
use render::{
    PreparedSlideAssembly, caller_font_files, prepare_render_context, render_failure,
    render_prepared_timeline_request,
};

mod xml_owner;

use xml_owner::{
    PPT_MC_NS, PptXmlOwner, PptXmlPolicy, PptXmlScopeDelta, ppt_shape_nonvisual_parent,
    ppt_xml_child_order, ppt_xml_frontiers,
};

mod validation;

use validation::relationship_requires_xml_reference;
pub use validation::{ValidationIssue, validate_presentation_file};

mod open;
mod package;

pub use package::PresentationPackageClass;

mod properties;

use properties::{MetadataEdit, property_child_ranges, property_part};

mod masters;
mod themes;

pub use themes::{Theme, ThemeFonts};
use themes::{edit_master_layout_relationship, portable_owned_fragment};

mod import;
mod import_notes;
mod import_resources;
mod import_tables;

pub use import::ImportSlideOptions;
use import::{
    TAGS_RELATIONSHIP, VML_DRAWING_RELATIONSHIP, duplicate_part_graph,
    preflight_carried_part_graph, preflight_internal_transfer_image,
};
use import_notes::stage_source_notes_appearance;
use import_resources::{
    imported_typefaces, stage_source_master_defaults, transfer_imported_property_resources,
};
use import_tables::{
    ImportTableStyleAttribute, close_import_table_style, import_table_style_part,
    import_table_styles,
};

mod media;
mod pictures;

pub(crate) use media::validate_semantic_picture;
pub use media::{
    EmbeddedMediaInput, MediaDiagnostic, MediaInfo, MediaLocation, MediaPlaybackSettings,
    MediaPoster, MediaSourceInput,
};
use media::{
    invalid_media_mutation, is_known_media_content_type, media_playback_settings,
    media_relationship_type, register_content_type, slide_relationship_ids,
};
pub use pictures::PictureImage;

mod charts;
mod smartart;

use charts::chart_internal_part;
pub use smartart::{DiagramPart, SmartArtInfo};
use smartart::{collect_smartart_frames, validate_smartart_relationship_roles};

mod fonts;

use fonts::{
    DecodedEmbeddedFont, EMBEDDED_FONT_CONTENT_TYPE, MAX_EMBEDDED_DECODED_BYTES,
    checked_embedded_font, font_model_error,
};
pub use fonts::{
    EmbeddedFont, EmbeddedFontFace, EmbeddedFontFormat, EmbeddedTypeface, FontEmbeddingLicense,
};

mod header_footer;

pub use header_footer::{
    HeaderFooter, HeaderFooterDate, HeaderFooterFlags, SLIDE_NUMBER_FIELD_TEXT, date_field_text,
};

use header_footer::part_field_identities;

mod comments;

mod hyperlinks;

#[cfg(any(feature = "default-template", feature = "render"))]
const DEFAULT_TEMPLATE: &[u8] = include_bytes!("../assets/default.pptx");

/// A result returned by the `oxoffice-pptx` read facade.
pub type Result<T> = std::result::Result<T, Error>;

/// A refused pair in an ordered transactional replacement batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplacementCountMismatch {
    pub index: usize,
    pub placeholder: String,
    pub expected: usize,
    pub found: usize,
}

/// An error opening, resolving, or serialising a presentation package.
#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Package(#[from] OpcError),

    #[error("semantic HTML/Markdown conversion error at {path}: {message}")]
    Html { path: String, message: String },

    #[cfg(feature = "default-template")]
    #[error("ODP conversion error in {part:?} at byte {offset}: {message}")]
    Odp {
        part: Option<String>,
        offset: u64,
        message: String,
    },

    #[cfg(feature = "render")]
    #[error("PDF conformance error: {0}")]
    Pdf(#[from] oxoffice_pdf::PdfError),

    #[cfg(feature = "render")]
    #[error("PDF import error on page {page:?} at operation {offset:?}: {message}")]
    PdfImport {
        page: Option<u32>,
        offset: Option<usize>,
        message: String,
    },

    #[error("presentation package has no officeDocument relationship")]
    MissingMainDocument,

    #[error(
        "refusing to save over {}, the file this presentation was opened from; pass overwrite_source to replace it",
        path.display()
    )]
    OverwritesSource { path: PathBuf },

    #[error("unsupported presentation main content type: {content_type:?}")]
    UnsupportedPackageClass { content_type: Option<String> },

    #[error("{source_part}: relationship {relationship_id} is missing")]
    MissingRelationship {
        source_part: String,
        relationship_id: String,
    },

    #[error("{source_part}: relationship {relationship_id} has type {actual}, expected {expected}")]
    WrongRelationshipType {
        source_part: String,
        relationship_id: String,
        expected: &'static str,
        actual: String,
    },

    #[error("{source_part}: relationship {relationship_id} has an external target")]
    ExternalRelationship {
        source_part: String,
        relationship_id: String,
    },

    #[error("package part is missing: {part_name}")]
    MissingPart { part_name: String },

    #[error("cannot create core properties at occupied package part {part_name}")]
    CorePropertiesPartCollision { part_name: String },

    #[error("cannot create collaboration data at occupied package part {part_name}")]
    CollaborationPartCollision { part_name: String },

    #[error(
        "{source_part}: found {count} relationships of type {relationship_type}, expected at most one"
    )]
    DuplicateRelationships {
        source_part: String,
        relationship_type: &'static str,
        count: usize,
    },

    #[error("{source_part}: found {count} notes-slide relationships, expected at most one")]
    DuplicateNotesSlides { source_part: String, count: usize },

    #[error("malformed PresentationML part {part_name}: {message}")]
    MalformedPart { part_name: String, message: String },

    #[error("layout index {index} is out of range for {layout_count} layouts")]
    UnknownLayoutIndex { index: usize, layout_count: usize },

    #[error("slide index {index} is out of range for {slide_count} slides")]
    UnknownSlideIndex { index: usize, slide_count: usize },

    #[error("picture {filename} has unsupported image bytes")]
    UnsupportedPicture { filename: String },

    #[error("picture {filename} has no usable native dimensions")]
    UnavailablePictureDimensions { filename: String },

    #[error("picture {filename} has an inferred {axis} outside the EMU range")]
    PictureDimensionOutOfRange {
        filename: String,
        axis: &'static str,
    },

    #[error("PowerPoint slide ids are exhausted")]
    SlideIdExhausted,

    #[error("{operation} is not supported for {shape_kind:?}")]
    UnsupportedShapeMutation {
        operation: &'static str,
        shape_kind: ShapeKind,
    },

    #[error("{operation} failed: {message}")]
    InvalidShapeMutation {
        operation: &'static str,
        message: String,
    },

    #[error("{operation} failed: {message}")]
    InvalidTableMutation {
        operation: &'static str,
        message: String,
    },

    #[error("{operation} failed: {message}")]
    InvalidPresentationMutation {
        operation: &'static str,
        message: String,
    },

    #[error("{operation} failed: {message}")]
    InvalidSlideMutation {
        operation: &'static str,
        message: String,
    },

    #[error("{operation} failed: {message}")]
    InvalidChartMutation {
        operation: &'static str,
        message: String,
    },

    #[error("{operation} failed: {message}")]
    InvalidMediaMutation {
        operation: &'static str,
        message: String,
    },

    #[error("{operation} failed: {message}")]
    InvalidEmbeddedMutation {
        operation: &'static str,
        message: String,
    },

    /// A font embedding input or byte budget was refused before mutation.
    #[error("{0}")]
    InvalidFontEmbedding(String),

    #[error("adjustment {name} requires a finite value")]
    NonFiniteAdjustmentValue { name: String },

    #[cfg(feature = "render")]
    #[error("presentation render failed: {message}")]
    Render { message: String },
}

/// An opened PresentationML package and its ordered slide read model.
#[derive(Clone, Debug)]
pub struct Presentation {
    package: OpcPackage,
    media_store: MediaStore,
    presentation_part: String,
    presentation: CT_Presentation,
    core_properties: Option<CoreProperties>,
    core_properties_part_name: String,
    core_properties_dirty: bool,
    application_properties: Option<AppProperties>,
    custom_properties: Option<CustomProperties>,
    comment_authors_part: Option<String>,
    comment_authors: CommentAuthorList,
    comment_authors_dirty: bool,
    notes_master_part: Option<String>,
    notes_master: Option<Arc<CT_NotesMaster>>,
    notes_master_dirty: bool,
    handout_master_part: Option<String>,
    handout_master: Option<Arc<CT_HandoutMaster>>,
    handout_master_dirty: bool,
    embedded_invalidated_signatures: HashSet<(String, String)>,
    package_signatures_invalidated: bool,
    // The file this presentation was opened from, canonical, which `save`
    // refuses to replace. Clones and reopened candidates carry it.
    source_path: Option<PathBuf>,
    // Slide, layout and master records are shared between clones and copied
    // on write, so a candidate copy costs one pointer per part and an edit
    // copies only its own part.
    layouts: Vec<Arc<LayoutRecord>>,
    masters: Vec<Arc<MasterRecord>>,
    themes: Vec<ThemeRecord>,
    slides: Vec<Arc<SlideRecord>>,
    // What the last opening of a package read from bytes parsed, which
    // staging and commits reuse. None for a package built in memory.
    parsed: Option<Arc<Parsed>>,
}

#[derive(Clone, Debug)]
struct LayoutRecord {
    part_name: String,
    layout: CT_SlideLayout,
    dirty: bool,
}

/// Mutable records use the actual package part as their identity.
#[derive(Clone, Debug)]
struct MasterRecord {
    id: Option<u32>,
    part_name: String,
    // An unprojectable retained master may still be opened and saved unchanged.
    master: std::result::Result<CT_SlideMaster, String>,
    dirty: bool,
}
#[derive(Clone, Debug)]
struct ThemeRecord {
    part_name: String,
    theme: CT_OfficeStyleSheet,
    dirty: bool,
}

/// What the last opening of a package read from bytes parsed: the package
/// as read, and the records parsed from its parts. Holding the records makes
/// a later edit of one copy it, so a record held here is still what parsing
/// the bytes this package holds for its part gives. Staging skips comparing
/// such a record with its part, and a commit reuses it, instead of parsing
/// the part again, when the staged package still shares those bytes.
#[derive(Debug)]
struct Parsed {
    package: OpcPackage,
    presentation: (String, CT_Presentation),
    slides: HashMap<String, Arc<SlideRecord>>,
    layouts: HashMap<String, Arc<LayoutRecord>>,
    masters: HashMap<String, Arc<MasterRecord>>,
    notes_master: Option<(String, Arc<CT_NotesMaster>)>,
    handout_master: Option<(String, Arc<CT_HandoutMaster>)>,
    // The text field identities of each XML part the package holds, but a
    // part whose XML a scan refuses, carried from what the last opening
    // parsed while the part bytes are the same.
    field_ids: HashMap<String, Vec<String>>,
}

impl Parsed {
    fn of(presentation: &Presentation, earlier: Option<&Parsed>) -> Self {
        fn by_part<T>(records: &[Arc<T>], part: impl Fn(&T) -> &str) -> HashMap<String, Arc<T>> {
            records
                .iter()
                .map(|record| (part(record).to_owned(), Arc::clone(record)))
                .collect()
        }
        Self {
            package: presentation.package.clone(),
            presentation: (
                presentation.presentation_part.clone(),
                presentation.presentation.clone(),
            ),
            slides: by_part(&presentation.slides, |record| &record.part_name),
            layouts: by_part(&presentation.layouts, |record| &record.part_name),
            masters: by_part(&presentation.masters, |record| &record.part_name),
            notes_master: presentation
                .notes_master_part
                .clone()
                .zip(presentation.notes_master.clone()),
            handout_master: presentation
                .handout_master_part
                .clone()
                .zip(presentation.handout_master.clone()),
            field_ids: presentation
                .package
                .parts
                .iter()
                .filter(|(name, _)| name.ends_with(".xml"))
                .filter_map(|(name, bytes)| {
                    let carried = earlier.and_then(|earlier| {
                        earlier
                            .field_ids
                            .get(name)
                            .filter(|_| earlier.same_bytes(&presentation.package, name))
                    });
                    let ids = match carried {
                        Some(ids) => ids.clone(),
                        None => part_field_identities(name, bytes).ok()?,
                    };
                    Some((name.clone(), ids))
                })
                .collect(),
        }
    }

    /// The text field identities of `part` of `package`, when it holds the
    /// bytes read here.
    fn field_ids(&self, package: &OpcPackage, part: &str) -> Option<&[String]> {
        let ids = self.field_ids.get(part)?;
        self.same_bytes(package, part).then_some(ids.as_slice())
    }

    /// Whether `package` holds for `part` the very bytes read for it here.
    /// Names are compared exactly: a package that opened or a write
    /// accepted holds no two equivalent part names.
    fn same_bytes(&self, package: &OpcPackage, part: &str) -> bool {
        matches!(
            (package.parts.get(part), self.package.parts.get(part)),
            (Some(now), Some(read)) if Arc::ptr_eq(now, read)
        )
    }

    /// The presentation model parsing `part` of `package` gives, when it
    /// holds the bytes read here.
    fn presentation(&self, package: &OpcPackage, part: &str) -> Option<&CT_Presentation> {
        let (parsed, presentation) = &self.presentation;
        (parsed == part && self.same_bytes(package, part)).then_some(presentation)
    }

    /// Whether `record` is the slide record parsed here for its part.
    fn holds_slide(&self, record: &Arc<SlideRecord>) -> bool {
        self.slides
            .get(&record.part_name)
            .is_some_and(|parsed| Arc::ptr_eq(parsed, record))
    }

    /// The record that opening slide `id` at `part` of `package` gives,
    /// when its part, relationships, notes and comments are those it was
    /// parsed from here.
    fn slide(&self, package: &OpcPackage, part: &str, id: u32) -> Option<Arc<SlideRecord>> {
        let record = self.slides.get(part)?;
        let relationships = package.part_rels.get(part)?;
        (record.id == id
            && self.same_bytes(package, part)
            && self
                .package
                .part_rels
                .get(part)
                .is_some_and(|parsed| parsed.items == relationships.items)
            && record
                .notes
                .as_ref()
                .is_none_or(|notes| self.same_bytes(package, &notes.part_name))
            && record
                .comments
                .as_ref()
                .is_none_or(|comments| self.same_bytes(package, &comments.part_name)))
        .then(|| Arc::clone(record))
    }

    fn layout(&self, package: &OpcPackage, part: &str) -> Option<Arc<LayoutRecord>> {
        let record = self.layouts.get(part)?;
        self.same_bytes(package, part).then(|| Arc::clone(record))
    }

    fn master(
        &self,
        package: &OpcPackage,
        part: &str,
        id: Option<u32>,
    ) -> Option<Arc<MasterRecord>> {
        let record = self.masters.get(part)?;
        (record.id == id && self.same_bytes(package, part)).then(|| Arc::clone(record))
    }

    fn notes_master(&self, package: &OpcPackage, part: &str) -> Option<Arc<CT_NotesMaster>> {
        let (parsed, master) = self.notes_master.as_ref()?;
        (parsed == part && self.same_bytes(package, part)).then(|| Arc::clone(master))
    }

    fn handout_master(&self, package: &OpcPackage, part: &str) -> Option<Arc<CT_HandoutMaster>> {
        let (parsed, master) = self.handout_master.as_ref()?;
        (parsed == part && self.same_bytes(package, part)).then(|| Arc::clone(master))
    }
}

/// A checked shape-tree owner. Indices follow each existing public inventory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShapeOwner {
    Slide(usize),
    Layout(usize),
    Master(usize),
}

#[derive(Default)]
struct PptXmlNames {
    values: Vec<String>,
    ids: HashMap<String, usize>,
    routes: HashMap<(usize, usize, usize, usize), usize>,
    physical_owners: HashMap<(usize, usize, usize), usize>,
    route_edges: Vec<(usize, usize, usize, usize)>,
}
#[derive(Default)]
struct PptXmlScope {
    bindings: HashMap<String, usize>,
    ignorable: HashSet<usize>,
}
struct PptXmlIsland {
    route: usize,
    owner: usize,
    source: std::ops::Range<usize>,
    meaning: std::ops::Range<usize>,
    message: &'static str,
}
#[derive(Default)]
struct PptXmlFrontiers {
    islands: Vec<PptXmlIsland>,
    meaning: Vec<(usize, usize)>,
    policies: Vec<PptXmlPolicy>,
}

#[derive(Clone, Debug)]
struct SlideRecord {
    id: u32,
    part_name: String,
    slide: CT_Slide,
    notes: Option<NotesRecord>,
    comments: Option<CommentRecord>,
}

#[derive(Clone, Debug)]
struct CommentRecord {
    part_name: String,
    comments: CommentList,
    dirty: bool,
}

#[derive(Clone, Debug)]
struct NotesRecord {
    part_name: String,
    notes: CT_NotesSlide,
}

#[derive(Clone, Debug)]
struct MediaEntry {
    part_name: String,
    // The same allocation as the package part, so a clone copies no media.
    bytes: Arc<Vec<u8>>,
    extension: String,
    content_type: String,
    newly_inserted: bool,
}

#[derive(Clone, Debug)]
struct MediaStore {
    by_id: HashMap<MediaId, Vec<MediaEntry>>,
    namer: MediaNamer,
    media_namer: MediaNamer,
}

impl Presentation {
    /// Returns the number of slides in presentation order.
    pub fn len(&self) -> usize {
        self.slides.len()
    }

    /// Returns whether the presentation has no slides.
    pub fn is_empty(&self) -> bool {
        self.slides.is_empty()
    }

    /// Returns one slide by zero-based presentation index.
    pub fn slide(&self, index: usize) -> Option<SlideRef<'_>> {
        self.slides.get(index).map(|record| slide_ref(record))
    }

    /// Returns one slide for in-place mutation by zero-based index.
    pub fn slide_mut(&mut self, index: usize) -> Option<SlideMut<'_>> {
        self.slides.get(index)?;
        Some(slide_mut(self, index))
    }

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

    /// Iterates slides in `p:sldIdLst` order.
    pub fn slides(&self) -> impl ExactSizeIterator<Item = SlideRef<'_>> {
        self.slides.iter().map(|record| slide_ref(record))
    }

    /// For each slide in order, the index in `earlier` of the slide whose
    /// record it still shares, or `None` for a slide changed or added since.
    ///
    /// A clone shares every slide record with its source until an edit
    /// copies one, so a caller holding both can find the slides an edit
    /// touched without comparing their content.
    #[doc(hidden)]
    pub fn unchanged_slides_since(&self, earlier: &Presentation) -> Vec<Option<usize>> {
        let origins = earlier
            .slides
            .iter()
            .enumerate()
            .map(|(index, record)| (Arc::as_ptr(record), index))
            .collect::<HashMap<_, _>>();
        self.slides
            .iter()
            .map(|record| origins.get(&Arc::as_ptr(record)).copied())
            .collect()
    }

    /// Whether every layout and master record is still the one of `earlier`,
    /// in the same order.
    #[doc(hidden)]
    pub fn unchanged_layouts_and_masters_since(&self, earlier: &Presentation) -> bool {
        fn shared<T>(records: &[Arc<T>], earlier: &[Arc<T>]) -> bool {
            records.len() == earlier.len()
                && records
                    .iter()
                    .zip(earlier)
                    .all(|(record, earlier)| Arc::ptr_eq(record, earlier))
        }
        shared(&self.layouts, &earlier.layouts) && shared(&self.masters, &earlier.masters)
    }

    /// Replaces literal text across regular DrawingML runs in every editable shape.
    ///
    /// Replacement text retains the formatting and opaque run content of the
    /// first matched run. Unmatched prefixes and suffixes stay in their
    /// original runs. Groups and table cells are traversed recursively, while
    /// preserved alternate-content branches remain untouched.
    pub fn replace_text(&mut self, placeholder: &str, value: &str) -> usize {
        if placeholder.is_empty() {
            return 0;
        }
        // Stored as the run text setters store it, so it can be saved.
        let value = &oxoffice_drawing::text::escape_invalid_xml_characters(value);
        // A slide without a match keeps its record, so a shared one is not
        // copied and callers can tell the changed slides by pointer identity.
        self.slides
            .iter_mut()
            .filter(|record| slide_record_has_text(record, placeholder))
            .map(|record| {
                replace_text_in_slide_record(Arc::make_mut(record), placeholder, value, true)
            })
            .sum()
    }

    /// Replaces literal slide and speaker-note text on a staged candidate.
    ///
    /// The candidate is serialized before it replaces the live presentation,
    /// so a malformed retained subtree cannot publish a partial mutation.
    pub fn try_replace_text(&mut self, placeholder: &str, value: &str) -> Result<usize> {
        require_replacement_placeholder(placeholder)?;
        let mut candidate = self.clone();
        let count = candidate.replace_text(placeholder, value);
        candidate.staged_package(false)?;
        *self = candidate;
        Ok(count)
    }

    /// Applies ordered literal pairs on one candidate and publishes once.
    ///
    /// Counts refer to each pair's current candidate, so pairs may cascade.
    /// A mismatch or serialization failure leaves the original unchanged.
    pub fn try_replace_all_expected(
        &mut self,
        pairs: &[(&str, &str, Option<usize>)],
    ) -> Result<std::result::Result<Vec<usize>, ReplacementCountMismatch>> {
        for (placeholder, _, _) in pairs {
            require_replacement_placeholder(placeholder)?;
        }
        if pairs.is_empty() {
            return Ok(Ok(Vec::new()));
        }
        let mut candidate = self.clone();
        let mut counts = Vec::with_capacity(pairs.len());
        for (index, &(placeholder, value, expected)) in pairs.iter().enumerate() {
            let found = candidate.replace_text(placeholder, value);
            if let Some(expected) = expected.filter(|expected| *expected != found) {
                return Ok(Err(ReplacementCountMismatch {
                    index,
                    placeholder: placeholder.to_owned(),
                    expected,
                    found,
                }));
            }
            counts.push(found);
        }
        candidate.staged_package(false)?;
        *self = candidate;
        Ok(Ok(counts))
    }

    /// Replaces literal text on one slide, and in its speaker notes when
    /// `notes` is true, and returns the count.
    ///
    /// Matching, formatting and traversal are those of
    /// [`Presentation::try_replace_text`], restricted to that slide. Other
    /// slides and their notes are never touched. The replacement runs on a
    /// copy of the slide and its notes, which must serialize before the copy
    /// replaces them. With `expect`, a different count also leaves the slide
    /// unchanged and is returned for the caller to compare.
    pub fn try_replace_slide_text(
        &mut self,
        slide_index: usize,
        placeholder: &str,
        value: &str,
        notes: bool,
        expect: Option<usize>,
    ) -> Result<usize> {
        self.require_slide_index(slide_index)?;
        require_replacement_placeholder(placeholder)?;
        let mut candidate = SlideRecord::clone(&self.slides[slide_index]);
        let value = &oxoffice_drawing::text::escape_invalid_xml_characters(value);
        let count = replace_text_in_slide_record(&mut candidate, placeholder, value, notes);
        if expect.is_some_and(|expected| expected != count) {
            return Ok(count);
        }
        candidate
            .slide
            .to_xml()
            .map_err(|error| Error::MalformedPart {
                part_name: candidate.part_name.clone(),
                message: error.to_string(),
            })?;
        if let Some(record) = candidate.notes.as_ref().filter(|_| notes) {
            record
                .notes
                .to_xml()
                .map_err(|error| Error::MalformedPart {
                    part_name: record.part_name.clone(),
                    message: error.to_string(),
                })?;
        }
        self.slides[slide_index] = Arc::new(candidate);
        Ok(count)
    }

    /// Returns the optional slide dimensions in EMUs.
    pub fn slide_size(&self) -> Option<(Emu, Emu)> {
        self.presentation
            .slide_size
            .as_ref()
            .map(|size| (size.cx, size.cy))
    }

    /// Replaces the slide dimensions after validating both values.
    ///
    /// A size type such as `screen16x9` names the old dimensions, so new ones
    /// drop it and the deck reads as a custom size.
    pub fn set_slide_size(&mut self, width: Emu, height: Emu) -> Result<()> {
        self.presentation
            .set_slide_size(width, height)
            .map_err(|error| Error::InvalidPresentationMutation {
                operation: "set slide size",
                message: error.to_string(),
            })
    }

    /// Reads the optional displayed start without flattening absent and zero.
    pub fn slide_number_start(&self) -> Result<Option<u32>> {
        self.presentation
            .first_slide_number()
            .map_err(|error| Error::MalformedPart {
                part_name: self.presentation_part.clone(),
                message: error.to_string(),
            })
    }

    /// Stages checked displayed numbering without changing physical slide IDs.
    pub fn set_slide_number_start(&mut self, value: Option<u32>) -> Result<()> {
        let start = value.unwrap_or(1);
        if let Some(last) = self.slides.len().checked_sub(1) {
            let last = u32::try_from(last)
                .map_err(|_| invalid_slide_mutation("set slide number start", "too many slides"))?;
            start.checked_add(last).ok_or_else(|| {
                invalid_slide_mutation("set slide number start", "displayed slide number overflow")
            })?;
        }
        if self.slide_number_start()? == value {
            return Ok(());
        }
        let mut staged = self.clone();
        staged.presentation.set_first_slide_number(value);
        self.commit_candidate(staged)
    }

    fn owner_part_name(&self, owner: ShapeOwner) -> Result<&str> {
        if let ShapeOwner::Slide(index) = owner {
            self.require_slide_index(index)?;
        }
        let part = match owner {
            ShapeOwner::Slide(index) => self
                .slides
                .get(index)
                .map(|record| record.part_name.as_str()),
            ShapeOwner::Layout(index) => self
                .layouts
                .get(index)
                .map(|record| record.part_name.as_str()),
            ShapeOwner::Master(index) => self
                .masters
                .get(index)
                .map(|record| record.part_name.as_str()),
        };
        part.ok_or_else(|| invalid_shape_mutation("part owner", format!("missing {owner:?}")))
    }

    fn owner_data(&self, owner: ShapeOwner) -> Result<&CT_CommonSlideData> {
        self.owner_part_name(owner)?;
        match owner {
            ShapeOwner::Slide(index) => Ok(&self.slides[index].slide.common_slide_data),
            ShapeOwner::Layout(index) => Ok(&self.layouts[index].layout.common_slide_data),
            ShapeOwner::Master(index) => self.masters[index]
                .master
                .as_ref()
                .map(|master| &master.common_slide_data)
                .map_err(|message| Error::MalformedPart {
                    part_name: self.masters[index].part_name.clone(),
                    message: message.clone(),
                }),
        }
    }

    fn owner_data_mut(&mut self, owner: ShapeOwner) -> Result<&mut CT_CommonSlideData> {
        self.owner_data(owner)?;
        match owner {
            ShapeOwner::Slide(index) => Ok(&mut Arc::make_mut(&mut self.slides[index])
                .slide
                .common_slide_data),
            ShapeOwner::Layout(index) => {
                let record = Arc::make_mut(&mut self.layouts[index]);
                record.dirty = true;
                Ok(&mut record.layout.common_slide_data)
            }
            ShapeOwner::Master(index) => {
                let record = Arc::make_mut(&mut self.masters[index]);
                record.dirty = true;
                record
                    .master
                    .as_mut()
                    .map(|master| &mut master.common_slide_data)
                    .map_err(|message| Error::MalformedPart {
                        part_name: record.part_name.clone(),
                        message: message.clone(),
                    })
            }
        }
    }

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

    /// Returns presentation sections in producer order.
    pub fn sections(&self) -> &[Section] {
        self.presentation.sections()
    }

    /// Replaces all presentation sections after validating stable slide ids.
    pub fn set_sections(&mut self, sections: Vec<Section>) -> Result<()> {
        let mut staged = self.clone();
        staged
            .presentation
            .set_sections(sections)
            .map_err(|error| invalid_presentation_mutation("set sections", error.to_string()))?;
        self.commit_candidate(staged)
    }

    fn require_slide_index(&self, index: usize) -> Result<()> {
        if index < self.slides.len() {
            Ok(())
        } else {
            Err(Error::UnknownSlideIndex {
                index,
                slide_count: self.slides.len(),
            })
        }
    }

    /// Removes one slide and its owned package graph by zero-based index.
    ///
    /// The package metadata stops naming it: `docProps/app.xml` counts the
    /// slides left and drops its title lists, unless the source-preserving
    /// patch cannot edit it (one outside its namespace, say), and the package
    /// thumbnail, which may picture it, goes.
    pub fn remove_slide(&mut self, index: usize) -> Result<()> {
        if index >= self.slides.len() {
            return Err(Error::UnknownSlideIndex {
                index,
                slide_count: self.slides.len(),
            });
        }
        let mut staged = self.clone();
        staged.remove_slide_in_place(index)?;
        self.commit_candidate(staged)
    }

    /// Moves one slide to an existing final zero-based index.
    ///
    /// In a deck with sections, a slide that lands next to a slide of its own
    /// section stays in it, so it can lead or end that section; otherwise it
    /// joins the section of the slide before it, else of the slide after it.
    pub fn move_slide(&mut self, from_index: usize, to_index: usize) -> Result<()> {
        let slide_count = self.slides.len();
        if from_index >= slide_count {
            return Err(Error::UnknownSlideIndex {
                index: from_index,
                slide_count,
            });
        }
        if to_index >= slide_count {
            return Err(Error::UnknownSlideIndex {
                index: to_index,
                slide_count,
            });
        }
        if from_index == to_index {
            return Ok(());
        }
        let slide = self.slides.remove(from_index);
        self.slides.insert(to_index, slide);
        let slide_id = self.presentation.slide_ids.remove(from_index);
        let id = slide_id.id;
        self.presentation.slide_ids.insert(to_index, slide_id);
        self.presentation.place_slide_in_sections(id);
        Ok(())
    }

    /// Replaces one slide's speaker-note text, creating its notes slide when absent.
    ///
    /// A new notes slide follows python-pptx: it relates to the notes master
    /// and the slide, and clones the master's slide image, body and slide
    /// number placeholders. A presentation without a notes master first
    /// receives a copy of the bundled template's notes master and its theme,
    /// which needs the `default-template` or `render` feature.
    pub fn set_notes_text(&mut self, slide_index: usize, text: &str) -> Result<()> {
        self.require_slide_index(slide_index)?;
        if self.slides[slide_index].notes.is_some() {
            return slide_mut(self, slide_index).set_notes_text(text);
        }
        let mut staged = self.clone();
        staged.add_notes_slide_in_place(slide_index)?;
        slide_mut(&mut staged, slide_index).set_notes_text(text)?;
        self.commit_candidate(staged)
    }

    fn ensure_notes_master_registration_in_place(&mut self) -> Result<()> {
        let master_part = self.notes_master_part.as_ref().ok_or_else(|| {
            footer_malformed(
                &self.presentation_part,
                "presentation has no notes master owner",
            )
        })?;
        let relationships = self
            .package
            .get_part_rels(&self.presentation_part)
            .ok_or_else(|| {
                footer_malformed(
                    &self.presentation_part,
                    "notes master has no global relationship",
                )
            })?;
        let global = relationships
            .items
            .iter()
            .filter(|relationship| relationship.rel_type == rel_types::NOTES_MASTER)
            .collect::<Vec<_>>();
        if global.len() != 1 {
            return Err(footer_malformed(
                &self.presentation_part,
                "notes master requires exactly one global relationship",
            ));
        }
        let relationship = global[0];
        reject_external(&self.presentation_part, relationship)?;
        if OpcPackage::resolve_rel_target(&self.presentation_part, &relationship.target)
            != *master_part
        {
            return Err(footer_malformed(
                &self.presentation_part,
                "notes master global relationship disagrees with its owner",
            ));
        }
        let before = self
            .presentation
            .to_xml()
            .map_err(|error| footer_malformed(&self.presentation_part, error.to_string()))?;
        let mut presentation = self.presentation.clone();
        presentation
            .ensure_notes_master_relationship(&relationship.id)
            .map_err(|error| footer_malformed(&self.presentation_part, error.to_string()))?;
        if presentation
            .to_xml()
            .map_err(|error| footer_malformed(&self.presentation_part, error.to_string()))?
            != before
        {
            for notes in self.slides.iter().filter_map(|slide| slide.notes.as_ref()) {
                let explicit_same_master = self
                    .package
                    .get_part_rels(&notes.part_name)
                    .is_some_and(|rels| {
                        let mut masters = rels
                            .items
                            .iter()
                            .filter(|rel| rel.rel_type == rel_types::NOTES_MASTER);
                        masters.next().is_some_and(|rel| {
                            matches!(rel.target_mode.as_deref(), None | Some("Internal"))
                                && OpcPackage::resolve_rel_target(&notes.part_name, &rel.target)
                                    == *master_part
                                && masters.next().is_none()
                        })
                    });
                if !explicit_same_master {
                    return Err(footer_malformed(
                        &self.presentation_part,
                        "registering a notes master would change existing notes inheritance",
                    ));
                }
            }
        }
        self.presentation = presentation;
        Ok(())
    }

    fn add_notes_slide_in_place(&mut self, slide_index: usize) -> Result<()> {
        const OPERATION: &str = "add notes slide";
        if self.notes_master.is_none() {
            self.add_default_notes_master_in_place()?;
        }
        self.ensure_notes_master_registration_in_place()?;
        let (Some(master_part), Some(master)) = (&self.notes_master_part, &self.notes_master)
        else {
            return Err(Error::InvalidSlideMutation {
                operation: OPERATION,
                message: "presentation has no notes master".to_owned(),
            });
        };
        let mut notes = CT_NotesSlide::from_xml(
            format!(
                r#"<p:notes xmlns:a="{A_NS}" xmlns:p="{P_NS}" xmlns:r="{R_NS}"><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:notes>"#
            )
            .as_bytes(),
        )
        .expect("canonical notes slide shell");
        let tree = &mut notes.common_slide_data.shape_tree;
        let mut shape_ids = ShapeIdAllocator::scan(tree);
        for child in &master.common_slide_data.shape_tree.children {
            let ShapeTreeChild::Shape(shape) = child else {
                continue;
            };
            let Some(placeholder) = &shape.placeholder else {
                continue;
            };
            let label = match placeholder.ph_type {
                Some(PhType::SlideImage) => "Slide Image Placeholder",
                Some(PhType::Body) => "Notes Placeholder",
                Some(PhType::SlideNumber) => "Slide Number Placeholder",
                _ => continue,
            };
            let id = shape_ids.allocate();
            let mut clone = CT_Shape::new_placeholder(id, placeholder.clone())
                .map_err(|error| invalid_slide_mutation(OPERATION, error.to_string()))?;
            clone
                .set_name(&format!("{label} {}", id - 1))
                .map_err(|error| invalid_slide_mutation(OPERATION, error.to_string()))?;
            if placeholder.ph_type != Some(PhType::Body) {
                clone.text_body = None;
            }
            tree.append_child(ShapeTreeChild::Shape(clone));
        }

        let slide_part = self.slides[slide_index].part_name.clone();
        let notes_part = MediaNamer::scan(
            "/ppt/notesSlides",
            "notesSlide",
            self.package.parts.keys().map(String::as_str),
        )
        .next_part_name("xml");
        let mut notes_relationships = Relationships::new();
        notes_relationships.add(
            rel_types::NOTES_MASTER,
            &relative_part_target(&notes_part, master_part),
        );
        notes_relationships.add(
            rel_types::SLIDE,
            &relative_part_target(&notes_part, &slide_part),
        );
        let mut slide_relationships = self
            .package
            .get_part_rels(&slide_part)
            .cloned()
            .unwrap_or_default();
        slide_relationships.add(
            rel_types::NOTES_SLIDE,
            &relative_part_target(&slide_part, &notes_part),
        );
        let notes_xml = notes
            .to_xml()
            .map_err(|error| invalid_slide_mutation(OPERATION, error.to_string()))?;
        self.package.set_part(&notes_part, notes_xml);
        self.package.set_part_rels(&notes_part, notes_relationships);
        self.package.set_part_rels(&slide_part, slide_relationships);
        self.package
            .content_types
            .add_override(&notes_part, content_types::NOTES_SLIDE);
        Arc::make_mut(&mut self.slides[slide_index]).notes = Some(NotesRecord {
            part_name: notes_part,
            notes,
        });
        Ok(())
    }

    #[cfg(any(feature = "default-template", feature = "render"))]
    fn add_default_notes_master_in_place(&mut self) -> Result<()> {
        let template = Self::from_bytes(DEFAULT_TEMPLATE)?;
        let (Some(template_master_part), Some(master)) =
            (template.notes_master_part, template.notes_master)
        else {
            return Err(invalid_slide_mutation(
                "add notes master",
                "the bundled template has no notes master",
            ));
        };
        let template_theme_part =
            related_internal_part(&template.package, &template_master_part, rel_types::THEME)?
                .ok_or_else(|| {
                    invalid_slide_mutation(
                        "add notes master",
                        "the bundled notes master has no theme",
                    )
                })?;
        let master_part = MediaNamer::scan(
            "/ppt/notesMasters",
            "notesMaster",
            self.package.parts.keys().map(String::as_str),
        )
        .next_part_name("xml");
        let theme_part = MediaNamer::scan(
            "/ppt/theme",
            "theme",
            self.package.parts.keys().map(String::as_str),
        )
        .next_part_name("xml");
        self.package.set_part(
            &master_part,
            required_part(&template.package, &template_master_part)?.to_vec(),
        );
        self.package.set_part(
            &theme_part,
            required_part(&template.package, &template_theme_part)?.to_vec(),
        );
        let mut master_relationships = Relationships::new();
        master_relationships.add(
            rel_types::THEME,
            &relative_part_target(&master_part, &theme_part),
        );
        self.package
            .set_part_rels(&master_part, master_relationships);
        self.package
            .content_types
            .add_override(&master_part, content_types::NOTES_MASTER);
        self.package
            .content_types
            .add_override(&theme_part, content_types::THEME);
        let mut presentation_relationships = self
            .package
            .get_part_rels(&self.presentation_part)
            .cloned()
            .unwrap_or_default();
        let relationship_id = presentation_relationships.add(
            rel_types::NOTES_MASTER,
            &relative_part_target(&self.presentation_part, &master_part),
        );
        self.presentation
            .ensure_notes_master_relationship(&relationship_id)
            .map_err(|error| footer_malformed(&self.presentation_part, error.to_string()))?;
        self.package
            .set_part_rels(&self.presentation_part, presentation_relationships);
        self.notes_master_part = Some(master_part);
        self.notes_master = Some(master);
        Ok(())
    }

    #[cfg(not(any(feature = "default-template", feature = "render")))]
    fn add_default_notes_master_in_place(&mut self) -> Result<()> {
        Err(invalid_slide_mutation(
            "add notes master",
            "presentation has no notes master and the bundled template is not compiled in",
        ))
    }

    /// Duplicates one slide immediately after its source.
    ///
    /// The copy has its own notes and its own modern comments, whose comment
    /// and reply ids are fresh and whose anchors follow the copy. Its legacy
    /// comments are copied into a part of its own, each taking its author's
    /// next index; a legacy reply whose parent comment is on another slide
    /// brings a copy of that parent along, as an import does.
    ///
    /// `docProps/app.xml` counts the new slide and drops its title lists,
    /// which PowerPoint writes again on its next save.
    ///
    /// In a deck with sections, the slide joins the section of the slide
    /// before it, else of the slide after it.
    pub fn duplicate_slide(&mut self, index: usize) -> Result<SlideRef<'_>> {
        if index >= self.slides.len() {
            return Err(Error::UnknownSlideIndex {
                index,
                slide_count: self.slides.len(),
            });
        }
        let mut staged = self.clone();
        let inserted = staged.duplicate_slide_in_place(index)?;
        staged.refresh_document_metadata(MetadataEdit::Addition)?;
        *self = staged;
        self.slides
            .get(inserted)
            .map(|record| slide_ref(record))
            .ok_or(Error::UnknownSlideIndex {
                index: inserted,
                slide_count: self.slides.len(),
            })
    }

    fn remove_slide_in_place(&mut self, index: usize) -> Result<()> {
        let record = self.slides[index].clone();
        self.presentation.remove_slide_from_sections(record.id);
        let presentation_relationship_id =
            self.presentation.slide_ids[index].relationship_id.clone();
        self.presentation
            .remove_custom_show_slide(&presentation_relationship_id)
            .map_err(|error| Error::MalformedPart {
                part_name: self.presentation_part.clone(),
                message: error.to_string(),
            })?;

        // Every part the slide or its notes reach is a candidate: charts with
        // their workbooks, legacy comments, embedded objects and media go
        // unless another part still references them.
        let mut candidates = HashSet::new();
        self.release_slide_jumps_to(&record.part_name, &mut candidates)?;
        collect_internal_targets(&self.package, &record.part_name, &mut candidates);
        if let Some(notes) = &record.notes {
            collect_internal_targets(&self.package, &notes.part_name, &mut candidates);
            self.package.remove_part(&notes.part_name);
            self.package.remove_part_rels(&notes.part_name);
            self.package.content_types.remove_override(&notes.part_name);
        }
        if let Some(comments) = &record.comments {
            self.package.remove_part(&comments.part_name);
            self.package.remove_part_rels(&comments.part_name);
            self.package
                .content_types
                .remove_override(&comments.part_name);
        }
        self.package.remove_part(&record.part_name);
        self.package.remove_part_rels(&record.part_name);
        self.package
            .content_types
            .remove_override(&record.part_name);
        if let Some(relationships) = self.package.get_part_rels_mut(&self.presentation_part) {
            relationships
                .items
                .retain(|relationship| relationship.id != presentation_relationship_id);
        }
        self.presentation.slide_ids.remove(index);
        self.slides.remove(index);
        prune_unreachable_parts(&mut self.package, &candidates);
        self.media_store = self.media_store.rescan(&self.package);
        self.refresh_document_metadata(MetadataEdit::Removal)
    }

    /// Makes every hyperlink of another slide that jumps to `slide_part` do
    /// nothing and removes its relationship, as PowerPoint does when the
    /// target slide is deleted.
    ///
    /// Every `a:hlinkClick`, `a:hlinkHover`, or `a:hlinkMouseOver` that
    /// named it, on a shape or a text run, keeps an empty `r:id` and the
    /// action `ppaction://noaction`. Every relationship that the slide named
    /// only inside a released element goes, a click sound included, and the
    /// internal targets of those relationships join `candidates` for pruning.
    fn release_slide_jumps_to(
        &mut self,
        slide_part: &str,
        candidates: &mut HashSet<String>,
    ) -> Result<()> {
        for record in &mut self.slides {
            let Some(relationships) = self.package.get_part_rels_mut(&record.part_name) else {
                continue;
            };
            let released: HashSet<String> = relationships
                .items
                .iter()
                .filter(|relationship| {
                    !relationship_is_external(relationship)
                        && OpcPackage::resolve_rel_target(&record.part_name, &relationship.target)
                            == slide_part
                })
                .map(|relationship| relationship.id.clone())
                .collect();
            if released.is_empty() || record.part_name == slide_part {
                continue;
            }
            let malformed = |error: OxmlError| Error::MalformedPart {
                part_name: record.part_name.clone(),
                message: error.to_string(),
            };
            let original = record.slide.to_xml().map_err(malformed)?;
            let xml = release_hyperlinks(&original, &released).map_err(malformed)?;
            let before: HashSet<String> = relationship_ids(&original)
                .map_err(malformed)?
                .into_iter()
                .collect();
            let after: HashSet<String> = relationship_ids(&xml)
                .map_err(malformed)?
                .into_iter()
                .collect();
            relationships.items.retain(|relationship| {
                let dropped = (released.contains(&relationship.id)
                    || before.contains(&relationship.id))
                    && !after.contains(&relationship.id);
                if dropped && !relationship_is_external(relationship) {
                    let target =
                        OpcPackage::resolve_rel_target(&record.part_name, &relationship.target);
                    if target != slide_part {
                        candidates.insert(target);
                    }
                }
                !dropped
            });
            let slide = CT_Slide::from_xml(&xml).map_err(malformed)?;
            Arc::make_mut(record).slide = slide;
        }
        Ok(())
    }

    fn duplicate_slide_in_place(&mut self, index: usize) -> Result<usize> {
        let source_package = self.package.clone();
        let source = SlideRecord::clone(&self.slides[index]);
        // Legacy comments are copied as an import copies them: a part of
        // the copy's own, whose comments take their authors' next indexes.
        let mut legacy_comments = HashMap::new();
        let source_slides = self.slides.clone();
        let presentation_part = self.presentation_part.clone();
        self.import_classic_comment_graphs(
            &source_slides,
            &presentation_part,
            &source_package,
            &[index],
            &mut legacy_comments,
        )?;
        let inserted = self.copy_slide_from_in_place(
            &source_package,
            source,
            index + 1,
            "duplicate slide",
            None,
            None,
        )?;
        let slide_part = self.slides[inserted].part_name.clone();
        if let Some(relationship) =
            self.package
                .get_part_rels_mut(&slide_part)
                .and_then(|relationships| {
                    relationships
                        .items
                        .iter_mut()
                        .find(|relationship| relationship.rel_type == rel_types::COMMENTS)
                })
            && let Some(copied) = legacy_comments.get(&OpcPackage::resolve_rel_target(
                &slide_part,
                &relationship.target,
            ))
        {
            relationship.target = relative_part_target(&slide_part, copied);
        }
        Ok(inserted)
    }

    /// Inserts a new slide at a final zero-based position, including append.
    ///
    /// Layout selection, creation and placement share one staged publication.
    pub fn add_slide_at(&mut self, layout_index: usize, position: usize) -> Result<usize> {
        if layout_index >= self.layouts.len() {
            return Err(Error::UnknownLayoutIndex {
                index: layout_index,
                layout_count: self.layouts.len(),
            });
        }
        let old_count = self.slides.len();
        let new_count = old_count.checked_add(1).ok_or(Error::SlideIdExhausted)?;
        if position >= new_count {
            return Err(Error::UnknownSlideIndex {
                index: position,
                slide_count: new_count,
            });
        }
        let mut candidate = self.clone();
        let id = candidate.add_slide(layout_index)?.id();
        // The new slide takes the section of where it lands, not of the end
        // of the deck it was appended to first.
        candidate.presentation.remove_slide_from_sections(id);
        candidate.move_slide(old_count, position)?;
        candidate.presentation.place_slide_in_sections(id);
        self.commit_candidate(candidate)?;
        Ok(position)
    }

    /// Synthesizes a new slide from one layout selected by zero-based index.
    ///
    /// `docProps/app.xml` counts the new slide and drops its title lists,
    /// which PowerPoint writes again on its next save.
    ///
    /// In a deck with sections, the slide joins the section of the slide
    /// before it, else of the slide after it.
    pub fn add_slide(&mut self, layout_index: usize) -> Result<SlideRef<'_>> {
        let layout = self
            .layouts
            .get(layout_index)
            .ok_or(Error::UnknownLayoutIndex {
                index: layout_index,
                layout_count: self.layouts.len(),
            })?;
        let layout_part = layout.part_name.clone();
        let placeholders = layout
            .layout
            .common_slide_data
            .shape_tree
            .children
            .iter()
            .filter_map(|child| match child {
                ShapeTreeChild::Shape(shape) => shape.placeholder.as_ref(),
                _ => None,
            })
            .filter(|placeholder| !is_latent_placeholder(placeholder))
            .cloned()
            .collect::<Vec<_>>();

        let mut shape_tree = CT_ShapeTree::new();
        let mut shape_ids = ShapeIdAllocator::scan(&shape_tree);
        for placeholder in placeholders {
            let id = shape_ids.allocate();
            let name = generated_placeholder_name(&shape_tree, &placeholder, id)?;
            let mut shape = CT_Shape::new_placeholder(id, placeholder).map_err(|error| {
                Error::MalformedPart {
                    part_name: "new slide".to_owned(),
                    message: error.to_string(),
                }
            })?;
            shape
                .set_name(&name)
                .map_err(|error| invalid_slide_mutation("add slide", error.to_string()))?;
            shape_tree.children.push(ShapeTreeChild::Shape(shape));
        }

        for shape in self.new_slide_latent_shapes(layout_index, &mut shape_ids)? {
            shape_tree.children.push(shape);
        }

        let slide_part = MediaNamer::scan(
            "/ppt/slides",
            "slide",
            self.package.parts.keys().map(String::as_str),
        )
        .next_part_name("xml");
        let slide = CT_Slide::new(shape_tree);
        let slide_xml = slide.to_xml().map_err(|error| Error::MalformedPart {
            part_name: slide_part.clone(),
            message: error.to_string(),
        })?;

        let id = self
            .presentation
            .slide_ids
            .iter()
            .map(|slide| slide.id)
            .max()
            .unwrap_or(255)
            .max(255)
            .checked_add(1)
            .ok_or(Error::SlideIdExhausted)?;

        let mut slide_relationships = Relationships::new();
        slide_relationships.add(
            rel_types::SLIDE_LAYOUT,
            &relative_part_target(&slide_part, &layout_part),
        );
        let mut presentation_relationships = self
            .package
            .get_part_rels(&self.presentation_part)
            .cloned()
            .unwrap_or_default();
        let presentation_relationship_id = presentation_relationships.add(
            rel_types::SLIDE,
            &relative_part_target(&self.presentation_part, &slide_part),
        );
        let slide_id = CT_SlideId::new(id, presentation_relationship_id).map_err(|error| {
            Error::MalformedPart {
                part_name: self.presentation_part.clone(),
                message: error.to_string(),
            }
        })?;

        self.package.set_part(&slide_part, slide_xml);
        self.package.set_part_rels(&slide_part, slide_relationships);
        self.package
            .set_part_rels(&self.presentation_part, presentation_relationships);
        self.package
            .content_types
            .add_override(&slide_part, content_types::SLIDE);
        self.presentation.slide_ids.push(slide_id);
        self.presentation.place_slide_in_sections(id);
        self.slides.push(Arc::new(SlideRecord {
            id,
            part_name: slide_part,
            slide,
            notes: None,
            comments: None,
        }));
        self.refresh_document_metadata(MetadataEdit::Addition)?;
        self.slides
            .last()
            .map(|record| slide_ref(record))
            .ok_or(Error::MalformedPart {
                part_name: self.presentation_part.clone(),
                message: "new slide record was not retained".to_owned(),
            })
    }

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

fn invalid_shape_mutation(operation: &'static str, message: impl Into<String>) -> Error {
    Error::InvalidShapeMutation {
        operation,
        message: message.into(),
    }
}

fn invalid_slide_mutation(operation: &'static str, message: impl Into<String>) -> Error {
    Error::InvalidSlideMutation {
        operation,
        message: message.into(),
    }
}

fn xml_local_name(name: &[u8]) -> &[u8] {
    name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
}

fn xml_is(namespace: &XmlResolveResult<'_>, expected: &str) -> bool {
    matches!(namespace, XmlResolveResult::Bound(XmlNamespace(uri)) if *uri == expected.as_bytes())
}

fn shape_id_count(children: &[ShapeTreeChild], shape_id: u32) -> usize {
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

fn detach_connectors(children: &mut [ShapeTreeChild], removed_ids: &HashSet<u32>) {
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

fn media_infos_for_slide(
    package: &OpcPackage,
    slide_index: usize,
    slide_part: &str,
    slide: &CT_Slide,
) -> Result<Vec<MediaInfo>> {
    let relationships = package
        .get_part_rels(slide_part)
        .cloned()
        .unwrap_or_default();
    slide
        .common_slide_data
        .shape_tree
        .children
        .iter()
        .filter_map(|child| match child {
            ShapeTreeChild::Picture(picture) => picture
                .media
                .as_ref()
                .zip(child.non_visual_id())
                .map(|(media, shape_id)| (picture, media, shape_id)),
            _ => None,
        })
        .map(|(picture, media, shape_id)| {
            let relationship_id = media.source.relationship_id();
            let relationship = relationships.get_by_id(relationship_id).ok_or_else(|| {
                Error::MissingRelationship {
                    source_part: slide_part.to_owned(),
                    relationship_id: relationship_id.to_owned(),
                }
            })?;
            if relationship.rel_type != rel_types::POWERPOINT_MEDIA
                && relationship.rel_type != media_relationship_type(media.kind)
            {
                return Err(Error::WrongRelationshipType {
                    source_part: slide_part.to_owned(),
                    relationship_id: relationship_id.to_owned(),
                    expected: rel_types::POWERPOINT_MEDIA,
                    actual: relationship.rel_type.clone(),
                });
            }
            let source = if relationship_is_external(relationship) {
                MediaLocation::Linked {
                    target: relationship.target.clone(),
                }
            } else {
                let part_name = OpcPackage::resolve_rel_target(slide_part, &relationship.target);
                required_part(package, &part_name)?;
                let content_type = package
                    .content_types
                    .content_type_for(&part_name)
                    .ok_or_else(|| {
                        invalid_media_mutation(
                            "inspect media",
                            format!("package part {part_name} has no content type"),
                        )
                    })?
                    .to_owned();
                MediaLocation::Embedded {
                    part_name,
                    content_type,
                }
            };
            let (settings, mut diagnostics) =
                media_playback_settings(picture, slide.timing.as_ref(), shape_id);
            let content_type = match &source {
                MediaLocation::Embedded { content_type, .. } => Some(content_type.as_str()),
                MediaLocation::Linked { .. } => None,
            };
            if let Some(content_type) = content_type
                && !is_known_media_content_type(content_type)
            {
                diagnostics.push(MediaDiagnostic::UnsupportedContentType {
                    content_type: content_type.to_owned(),
                });
            }
            if media.poster_relationship_id.is_none() {
                diagnostics.push(MediaDiagnostic::MissingPoster);
            }
            Ok(MediaInfo {
                slide_index,
                shape_id,
                kind: media.kind,
                source,
                poster_relationship_id: media.poster_relationship_id.clone(),
                settings,
                diagnostics,
            })
        })
        .collect()
}

struct ImportTableStyles {
    metadata: Vec<ImportTableStyleAttribute>,
    opaque: Vec<Vec<u8>>,
    default: Option<String>,
    styles: Vec<(String, Vec<u8>)>,
    namespaces: Vec<(String, String)>,
    close: usize,
    insert: usize,
    head_end: usize,
    name: String,
    empty: bool,
}

fn import_namespace_uri(
    namespace: XmlResolveResult<'_>,
    decoder: quick_xml::encoding::Decoder,
) -> std::result::Result<String, OxmlError> {
    match namespace {
        XmlResolveResult::Bound(uri) => {
            let attribute = quick_xml::events::attributes::Attribute {
                key: quick_xml::name::QName(b"xmlns"),
                value: std::borrow::Cow::Borrowed(uri.as_ref()),
            };
            Ok(attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder)?
                .into_owned())
        }
        _ => Ok(String::new()),
    }
}

fn fresh_related_part_name(package: &OpcPackage, source_part: &str) -> String {
    let (directory, filename) = source_part.rsplit_once('/').unwrap_or(("", source_part));
    let (stem_with_suffix, extension) = filename.rsplit_once('.').unwrap_or((filename, "xml"));
    let stem = stem_with_suffix.trim_end_matches(|character: char| character.is_ascii_digit());
    MediaNamer::scan(
        &format!("/{}", directory.trim_start_matches('/')),
        if stem.is_empty() { "part" } else { stem },
        package.parts.keys().map(String::as_str),
    )
    .next_part_name(extension)
}

fn require_internal_related_part(
    package: &OpcPackage,
    source_part: &str,
    relationship_id: &str,
    expected_type: &'static str,
) -> Result<String> {
    let relationship = package
        .get_part_rels(source_part)
        .and_then(|items| items.get_by_id(relationship_id))
        .ok_or_else(|| Error::MissingRelationship {
            source_part: source_part.to_owned(),
            relationship_id: relationship_id.to_owned(),
        })?;
    require_relationship_type(source_part, relationship, expected_type)?;
    reject_external(source_part, relationship)?;
    let target = OpcPackage::resolve_rel_target(source_part, &relationship.target);
    required_part(package, &target)?;
    Ok(target)
}

fn resolve_diagram_part<T>(
    package: &OpcPackage,
    source_part: &str,
    relationship_id: &str,
    expected_type: &'static str,
    parse: fn(&[u8]) -> std::result::Result<T, OxmlError>,
) -> DiagramPart<T> {
    let Some(relationship) = package
        .get_part_rels(source_part)
        .and_then(|items| items.get_by_id(relationship_id))
    else {
        return DiagramPart::MissingTarget(relationship_id.to_owned());
    };
    if relationship.rel_type != expected_type {
        return DiagramPart::Invalid(format!(
            "relationship {relationship_id} has type {}, expected {expected_type}",
            relationship.rel_type
        ));
    }
    if relationship_is_external(relationship) {
        return DiagramPart::External(relationship.target.clone());
    }
    let target = OpcPackage::resolve_rel_target(source_part, &relationship.target);
    let Some(xml) = package.get_part(&target) else {
        return DiagramPart::MissingTarget(target);
    };
    match parse(xml) {
        Ok(part) => DiagramPart::Parsed(part),
        Err(error) => DiagramPart::Invalid(format!("{target}: {error}")),
    }
}

fn related_internal_part(
    package: &OpcPackage,
    source_part: &str,
    relationship_type: &str,
) -> Result<Option<String>> {
    let Some(relationship) = package
        .get_part_rels(source_part)
        .and_then(|relationships| relationships.get_by_type(relationship_type))
    else {
        return Ok(None);
    };
    reject_external(source_part, relationship)?;
    Ok(Some(OpcPackage::resolve_rel_target(
        source_part,
        &relationship.target,
    )))
}

/// Adds every internal target of `source_part` to `targets`, as candidates
/// for [`prune_unreachable_parts`], which keeps those still referenced.
fn collect_internal_targets(
    package: &OpcPackage,
    source_part: &str,
    targets: &mut HashSet<String>,
) {
    let Some(relationships) = package.get_part_rels(source_part) else {
        return;
    };
    targets.extend(
        relationships
            .items
            .iter()
            .filter(|relationship| !relationship_is_external(relationship))
            .map(|relationship| OpcPackage::resolve_rel_target(source_part, &relationship.target)),
    );
}

/// Removes candidate parts that no relationship reaches any longer.
///
/// A removed part's own internal targets become candidates in turn, so a
/// removed chart also drops its embedded workbook.
/// Removes the candidate parts nothing references any more, and the parts
/// only they referenced. Returns whether any part was removed.
fn prune_unreachable_parts(package: &mut OpcPackage, candidates: &HashSet<String>) -> bool {
    let mut removed = false;
    let mut pending = candidates.iter().cloned().collect::<Vec<_>>();
    pending.sort_unstable();
    while let Some(candidate) = pending.pop() {
        // Resolving keeps the last segment of a target unless it is empty or
        // a dot segment, so any other target with a different file name
        // cannot name the candidate and is not resolved.
        let file_name = candidate.rsplit('/').next().unwrap_or_default();
        let targets = |relationship: &Relationship| {
            !relationship_is_external(relationship)
                && match relationship.target.rsplit('/').next().unwrap_or_default() {
                    "" | "." | ".." => true,
                    last => last.eq_ignore_ascii_case(file_name),
                }
        };
        let reachable_from_root = package.package_rels.items.iter().any(|relationship| {
            targets(relationship)
                && OpcPackage::resolve_rel_target("/", &relationship.target)
                    .eq_ignore_ascii_case(&candidate)
        });
        let reachable_from_part = package
            .part_rels
            .iter()
            .any(|(source_part, relationships)| {
                relationships.items.iter().any(|relationship| {
                    targets(relationship)
                        && OpcPackage::resolve_rel_target(source_part, &relationship.target)
                            .eq_ignore_ascii_case(&candidate)
                })
            });
        if reachable_from_root || reachable_from_part {
            continue;
        }
        if let Some(relationships) = package.get_part_rels(&candidate) {
            pending.extend(
                relationships
                    .items
                    .iter()
                    .filter(|relationship| !relationship_is_external(relationship))
                    .map(|relationship| {
                        OpcPackage::resolve_rel_target(&candidate, &relationship.target)
                    }),
            );
        }
        package.remove_part(&candidate);
        package.remove_part_rels(&candidate);
        package.content_types.remove_override(&candidate);
        removed = true;
    }
    removed
}

fn relationship_is_external(relationship: &Relationship) -> bool {
    relationship
        .target_mode
        .as_deref()
        .is_some_and(|mode| mode.eq_ignore_ascii_case("external"))
}

fn generated_placeholder_name(
    tree: &CT_ShapeTree,
    placeholder: &CT_Placeholder,
    id: u32,
) -> Result<String> {
    let base = match placeholder.ph_type.as_ref().unwrap_or(&PhType::Object) {
        PhType::Title | PhType::CenteredTitle => "Title",
        PhType::Subtitle => "Subtitle",
        PhType::Body => "Text Placeholder",
        PhType::Object => "Content Placeholder",
        PhType::Picture => "Picture Placeholder",
        PhType::Chart => "Chart Placeholder",
        PhType::Table => "Table Placeholder",
        PhType::Diagram => "SmartArt Placeholder",
        PhType::Media => "Media Placeholder",
        PhType::ClipArt => "ClipArt Placeholder",
        PhType::DateTime => "Date Placeholder",
        PhType::Footer => "Footer Placeholder",
        PhType::SlideNumber => "Slide Number Placeholder",
        PhType::Header => "Header Placeholder",
        PhType::SlideImage => "Slide Image Placeholder",
        PhType::VerticalTitle => "Vertical Title",
        PhType::VerticalBody => "Vertical Text Placeholder",
        PhType::VerticalObject => "Vertical Content Placeholder",
        PhType::Other(_) => "Placeholder",
    };
    let vertical = placeholder.orientation() == Some("vert") && !base.starts_with("Vertical ");
    // Unknown imported type naming keeps the existing generic ID convention.
    let mut ordinal = if matches!(placeholder.ph_type, Some(PhType::Other(_))) {
        id
    } else {
        id.saturating_sub(1)
    };
    loop {
        let name = format!(
            "{}{base} {ordinal}",
            if vertical { "Vertical " } else { "" }
        );
        if !tree
            .children
            .iter()
            .any(|child| child.non_visual_name().as_deref() == Some(name.as_str()))
        {
            return Ok(name);
        }
        ordinal = ordinal.checked_add(1).ok_or_else(|| {
            invalid_slide_mutation("add slide", "placeholder name space exhausted")
        })?;
    }
}

fn is_latent_placeholder(placeholder: &CT_Placeholder) -> bool {
    matches!(
        placeholder.ph_type.as_ref(),
        Some(PhType::DateTime | PhType::Footer | PhType::SlideNumber)
    )
}

fn presentation_embedded_fonts(
    package: &OpcPackage,
    presentation_part: &str,
    presentation: &CT_Presentation,
) -> Result<Vec<EmbeddedTypeface>> {
    let relationships = package.get_part_rels(presentation_part);
    let mut decoded: HashMap<String, DecodedEmbeddedFont> = HashMap::new();
    let mut decoded_bytes = 0usize;
    let mut entries = Vec::new();
    for entry in presentation.embedded_fonts() {
        let mut faces = Vec::new();
        for (kind, id) in entry.faces() {
            let part_name = relationships
                .and_then(|rels| rels.get_by_id(id))
                .filter(|rel| rel.rel_type == rel_types::FONT && !relationship_is_external(rel))
                .map(|rel| OpcPackage::resolve_rel_target(presentation_part, &rel.target));
            let stored = part_name.as_deref().and_then(|name| package.get_part(name));
            let mut format = EmbeddedFontFormat::Unknown;
            let mut data = None;
            if let (Some(name), Some(bytes)) = (&part_name, stored)
                && package.content_types.content_type_for(name) == Some(EMBEDDED_FONT_CONTENT_TYPE)
            {
                let cache_key = name.to_ascii_lowercase();
                if !decoded.contains_key(&cache_key) {
                    let (kind, body) = checked_embedded_font(bytes);
                    let shared = if let Some(body) = body {
                        decoded_bytes = decoded_bytes.checked_add(body.len()).ok_or_else(|| {
                            Error::InvalidFontEmbedding(
                                "decoded font byte budget overflow".to_owned(),
                            )
                        })?;
                        if decoded_bytes > MAX_EMBEDDED_DECODED_BYTES {
                            return Err(Error::InvalidFontEmbedding(
                                "decoded fonts exceed 256 MiB".to_owned(),
                            ));
                        }
                        Some(std::sync::Arc::<[u8]>::from(body))
                    } else {
                        None
                    };
                    decoded.insert(cache_key.clone(), (kind, shared));
                }
                (format, data) = decoded[&cache_key].clone();
            }
            faces.push(EmbeddedFontFace {
                kind,
                relationship_id: id.to_owned(),
                part_name,
                format,
                stored_len: stored.map_or(0, <[u8]>::len),
                data,
            });
        }
        entries.push(EmbeddedTypeface {
            typeface: entry.typeface().to_owned(),
            panose: entry.panose().map(str::to_owned),
            pitch_family: entry.pitch_family(),
            charset: entry.charset(),
            faces,
        });
    }
    Ok(entries)
}

fn relative_part_target(source_part: &str, target_part: &str) -> String {
    let mut source = source_part
        .trim_start_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    source.pop();
    let target = target_part
        .trim_start_matches('/')
        .split('/')
        .collect::<Vec<_>>();
    let common = source
        .iter()
        .zip(&target)
        .take_while(|(left, right)| left == right)
        .count();
    std::iter::repeat_n("..", source.len() - common)
        .chain(target[common..].iter().copied())
        .collect::<Vec<_>>()
        .join("/")
}

fn required_part<'a>(package: &'a OpcPackage, part_name: &str) -> Result<&'a [u8]> {
    package
        .get_part(part_name)
        .ok_or_else(|| Error::MissingPart {
            part_name: part_name.to_owned(),
        })
}

fn require_relationship_type(
    source_part: &str,
    relationship: &Relationship,
    expected: &'static str,
) -> Result<()> {
    if relationship.rel_type == expected {
        return Ok(());
    }
    Err(Error::WrongRelationshipType {
        source_part: source_part.to_owned(),
        relationship_id: relationship.id.clone(),
        expected,
        actual: relationship.rel_type.clone(),
    })
}

fn reject_external(source_part: &str, relationship: &Relationship) -> Result<()> {
    if relationship
        .target_mode
        .as_deref()
        .is_some_and(|mode| mode.eq_ignore_ascii_case("external"))
    {
        return Err(Error::ExternalRelationship {
            source_part: source_part.to_owned(),
            relationship_id: relationship.id.clone(),
        });
    }
    Ok(())
}

fn invalid_presentation_mutation(operation: &'static str, message: String) -> Error {
    Error::InvalidPresentationMutation { operation, message }
}

fn resolve_notes(package: &OpcPackage, slide_part: &str) -> Result<Option<NotesRecord>> {
    let Some(relationships) = package.get_part_rels(slide_part) else {
        return Ok(None);
    };
    let notes_relationships = relationships.get_all_by_type(rel_types::NOTES_SLIDE);
    if notes_relationships.len() > 1 {
        return Err(Error::DuplicateNotesSlides {
            source_part: slide_part.to_owned(),
            count: notes_relationships.len(),
        });
    }
    let Some(relationship) = notes_relationships.first() else {
        return Ok(None);
    };
    reject_external(slide_part, relationship)?;
    let part_name = OpcPackage::resolve_rel_target(slide_part, &relationship.target);
    let xml = required_part(package, &part_name)?;
    let notes = CT_NotesSlide::from_xml(xml).map_err(|error| Error::MalformedPart {
        part_name: part_name.clone(),
        message: error.to_string(),
    })?;
    Ok(Some(NotesRecord { part_name, notes }))
}

/// A borrowed slide in presentation order.
#[derive(Clone, Copy)]
pub struct SlideRef<'a> {
    record: &'a SlideRecord,
}

fn slide_ref(record: &SlideRecord) -> SlideRef<'_> {
    SlideRef { record }
}

/// A mutably borrowed slide in presentation order.
pub struct SlideMut<'a> {
    presentation: &'a mut Presentation,
    index: usize,
}

fn slide_mut(presentation: &mut Presentation, index: usize) -> SlideMut<'_> {
    SlideMut {
        presentation,
        index,
    }
}

impl<'a> SlideMut<'a> {
    /// Sets whether the slide is hidden from the slideshow.
    pub fn set_hidden(&mut self, hidden: bool) {
        if self.presentation.slides[self.index].slide.hidden() == hidden {
            return;
        }
        Arc::make_mut(&mut self.presentation.slides[self.index])
            .slide
            .set_hidden(hidden);
        // Only the hidden-slide count changes. Metadata that cannot be
        // refreshed is left as it was: it is no reason to refuse a hide.
        let _ = self
            .presentation
            .refresh_document_metadata(MetadataEdit::Visibility);
    }

    /// Sets a direct DrawingML fill without replacing a theme reference.
    pub fn set_background(&mut self, fill: Fill) -> Result<()> {
        Arc::make_mut(&mut self.presentation.slides[self.index])
            .slide
            .set_background(fill)
            .map_err(|error| Error::InvalidSlideMutation {
                operation: "set slide background",
                message: error.to_string(),
            })
    }

    /// Removes only a direct-fill background.
    pub fn clear_background(&mut self) {
        Arc::make_mut(&mut self.presentation.slides[self.index])
            .slide
            .clear_background();
    }

    /// Removes any slide background, direct fill or theme reference, so the
    /// slide follows its layout and master background.
    pub fn remove_background(&mut self) {
        Arc::make_mut(&mut self.presentation.slides[self.index])
            .slide
            .common_slide_data
            .background = None;
    }

    /// Replaces speaker-note text while preserving its body placeholder.
    pub fn set_notes_text(&mut self, text: &str) -> Result<()> {
        let notes = Arc::make_mut(&mut self.presentation.slides[self.index])
            .notes
            .as_mut()
            .ok_or_else(|| Error::InvalidSlideMutation {
                operation: "set notes text",
                message: "slide has no notes part".to_owned(),
            })?;
        let mut candidate = notes.notes.clone();
        let body = first_notes_body_mut(&mut candidate.common_slide_data.shape_tree.children)
            .ok_or_else(|| Error::InvalidSlideMutation {
                operation: "set notes text",
                message: "notes slide has no body placeholder".to_owned(),
            })?;
        let formatting_run = body.paragraphs().iter().find_map(|paragraph| {
            paragraph.runs.iter().find_map(|run| match run {
                TextRun::Run(run) => Some(run.clone()),
                TextRun::Break(_) | TextRun::Field(_) => None,
            })
        });
        // An empty first line would drop the formatting run before later
        // paragraphs copy it. A space keeps that copy, and the line loop
        // writes the caller's text afterwards.
        let seeded = (formatting_run.is_some() && (text.is_empty() || text.starts_with('\n')))
            .then(|| {
                text.split('\n')
                    .map(|line| if line.is_empty() { " " } else { line })
                    .collect::<Vec<_>>()
                    .join("\n")
            });
        body.set_text(seeded.as_deref().unwrap_or(text));
        // The general DrawingML setter treats CRLF as a paragraph separator
        // and drops every run when a line is empty. Notes authoring retains
        // the caller's exact line endings and the first regular run.
        for (index, line) in text.split('\n').enumerate() {
            let paragraph =
                body.paragraph_mut(index)
                    .ok_or_else(|| Error::InvalidSlideMutation {
                        operation: "set notes text",
                        message: "notes paragraph count changed during staging".to_owned(),
                    })?;
            assign_notes_line(paragraph, line, formatting_run.as_ref());
        }
        candidate
            .to_xml()
            .map_err(|error| Error::InvalidSlideMutation {
                operation: "set notes text",
                message: error.to_string(),
            })?;
        notes.notes = candidate;
        Ok(())
    }

    /// Appends a rectangular table at the top of the slide's z-order.
    pub fn add_table(
        &mut self,
        rows: usize,
        columns: usize,
        left: Emu,
        top: Emu,
        width: Emu,
        height: Emu,
    ) -> Result<ShapeMut<'_>> {
        self.append_member(|id| table_member(id, rows, columns, left, top, width, height))
    }

    /// Appends a no-fill textbox at the top of the slide's z-order.
    pub fn add_textbox(
        &mut self,
        left: Emu,
        top: Emu,
        width: Emu,
        height: Emu,
    ) -> Result<ShapeMut<'_>> {
        self.append_member(|id| textbox_member(id, left, top, width, height))
    }

    /// Appends an ordinary preset shape at the top of the slide's z-order.
    ///
    /// The shape carries the theme style python-pptx writes, so it shows the
    /// theme's accent fill, line, and effect with light text until direct
    /// properties replace them.
    pub fn add_shape(
        &mut self,
        preset: &str,
        left: Emu,
        top: Emu,
        width: Emu,
        height: Emu,
    ) -> Result<ShapeMut<'_>> {
        self.append_member(|id| preset_member(id, preset, left, top, width, height))
    }

    /// Appends a preset shape without a theme style for HTML and ODP imports.
    #[cfg(feature = "default-template")]
    pub(crate) fn add_unstyled_shape(
        &mut self,
        preset: &str,
        left: Emu,
        top: Emu,
        width: Emu,
        height: Emu,
    ) -> Result<ShapeMut<'_>> {
        self.append_member(|id| unstyled_preset_member(id, preset, left, top, width, height))
    }

    /// Appends a free-standing connector at the top of the slide's z-order.
    ///
    /// The connector carries the theme style python-pptx writes, so it shows
    /// the theme's line until a direct line replaces it.
    pub fn add_connector(
        &mut self,
        connector: ConnectorType,
        begin_x: Emu,
        begin_y: Emu,
        end_x: Emu,
        end_y: Emu,
    ) -> Result<ShapeMut<'_>> {
        self.append_member(|id| connector_member(id, connector, begin_x, begin_y, end_x, end_y))
    }

    /// Appends an empty group at the top of the slide's z-order.
    pub fn add_group_shape(&mut self) -> Result<ShapeMut<'_>> {
        self.append_member(|id| Ok(group_member(id, false)))
    }

    fn shape_tree(&mut self) -> &mut CT_ShapeTree {
        &mut Arc::make_mut(&mut self.presentation.slides[self.index])
            .slide
            .common_slide_data
            .shape_tree
    }

    /// Returns one immediate z-order child as a read-only handle.
    pub fn shape(&self, index: usize) -> Option<ShapeRef<'_>> {
        self.presentation.slides[self.index]
            .slide
            .common_slide_data
            .shape_tree
            .children
            .get(index)
            .map(shape_ref)
    }

    /// Returns one immediate z-order child for in-place mutation.
    pub fn shape_mut(&mut self, index: usize) -> Option<ShapeMut<'_>> {
        self.shape_tree().children.get(index)?;
        Some(owned_shape_mut(
            self.presentation,
            ShapeOwner::Slide(self.index),
            vec![index],
        ))
    }

    /// Consumes this handle and returns one immediate z-order child for mutation.
    pub fn into_shape_mut(self, index: usize) -> Option<ShapeMut<'a>> {
        self.presentation.slides[self.index]
            .slide
            .common_slide_data
            .shape_tree
            .children
            .get(index)?;
        Some(owned_shape_mut(
            self.presentation,
            ShapeOwner::Slide(self.index),
            vec![index],
        ))
    }
    fn append_member(
        &mut self,
        build: impl FnOnce(u32) -> Result<ShapeTreeChild>,
    ) -> Result<ShapeMut<'_>> {
        let owner = ShapeOwner::Slide(self.index);
        if self.presentation.local_append_eligible(owner, &[])? {
            let index = self.presentation.append_locally(owner, &[], build)?;
            return Ok(owned_shape_mut(self.presentation, owner, vec![index]));
        }
        let mut candidate = self.presentation.clone();
        let tree = &mut Arc::make_mut(&mut candidate.slides[self.index])
            .slide
            .common_slide_data
            .shape_tree;
        let index = append_new_member(tree, &[], build)?;
        candidate.refresh_connector_attachments(
            ShapeOwner::Slide(self.index),
            &self.presentation.owner_data(owner)?.shape_tree.children,
            &HashSet::new(),
        )?;
        candidate.slides[self.index]
            .slide
            .to_xml()
            .map_err(|error| Error::MalformedPart {
                part_name: candidate.slides[self.index].part_name.clone(),
                message: error.to_string(),
            })?;
        *self.presentation = candidate;
        Ok(owned_shape_mut(
            self.presentation,
            ShapeOwner::Slide(self.index),
            vec![index],
        ))
    }
}

/// The shapes of one slide, or of one group on it, borrowed for adding
/// shapes.
///
/// A new shape takes a `p:cNvPr` id unused across the whole slide, groups
/// included, and lands at the top of its collection's z-order. Coordinates
/// are in the collection's own space, which for a group is its member space.
/// An addition to a group refits that group and every group enclosing it to
/// the union of their members, as python-pptx does.
pub struct ShapesMut<'a> {
    presentation: &'a mut Presentation,
    owner: ShapeOwner,
    group: Vec<usize>,
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

fn textbox_member(id: u32, left: Emu, top: Emu, width: Emu, height: Emu) -> Result<ShapeTreeChild> {
    CT_Shape::new_textbox(
        id,
        &format!("TextBox {id}"),
        sized_transform("add textbox", left, top, width, height)?,
    )
    .map(ShapeTreeChild::Shape)
    .map_err(|error| invalid_shape_construction("add textbox", error))
}

fn preset_member(
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

#[cfg(feature = "default-template")]
fn unstyled_preset_member(
    id: u32,
    preset: &str,
    left: Emu,
    top: Emu,
    width: Emu,
    height: Emu,
) -> Result<ShapeTreeChild> {
    CT_Shape::new_preset(
        id,
        &format!("Shape {id}"),
        preset,
        sized_transform("add shape", left, top, width, height)?,
    )
    .map(ShapeTreeChild::Shape)
    .map_err(|error| invalid_shape_construction("add shape", error))
}

fn connector_member(
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
fn group_member(id: u32, nested: bool) -> ShapeTreeChild {
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

fn table_member(
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
    fn local_append_eligible(&self, owner: ShapeOwner, group: &[usize]) -> Result<bool> {
        if !group.is_empty() {
            return Ok(false);
        }
        Ok(!tree_has_connector(
            &self.owner_data(owner)?.shape_tree.children,
        ))
    }

    fn append_locally(
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
fn append_new_member(
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
fn append_member<'t>(
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
fn group_at_mut<'t>(
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
fn member_box(child: &ShapeTreeChild) -> Option<(CT_Point2D, CT_PositiveSize2D)> {
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
fn fit_group_to_members(group: &mut CT_GroupShape, occupied: bool) {
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

/// Builds a positioned transform for authored shape dimensions, refusing a
/// negative width or height at the call.
fn sized_transform(
    operation: &'static str,
    left: Emu,
    top: Emu,
    width: Emu,
    height: Emu,
) -> Result<CT_Transform2D> {
    if width.0 < 0 || height.0 < 0 {
        return Err(Error::InvalidShapeMutation {
            operation,
            message: format!(
                "shape width and height must not be negative, got {} by {} EMU",
                width.0, height.0
            ),
        });
    }
    Ok(positioned_transform(left, top, width, height))
}

fn positioned_transform(left: Emu, top: Emu, width: Emu, height: Emu) -> CT_Transform2D {
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

fn invalid_shape_construction(operation: &'static str, error: OxmlError) -> Error {
    Error::InvalidShapeMutation {
        operation,
        message: error.to_string(),
    }
}

fn invalid_table_mutation(operation: &'static str, message: String) -> Error {
    Error::InvalidTableMutation { operation, message }
}

impl<'a> SlideRef<'a> {
    /// Returns the producer-assigned slide id.
    pub fn id(&self) -> u32 {
        self.record.id
    }

    /// Returns the optional `p:cSld` name.
    pub fn name(&self) -> Option<&str> {
        self.record.slide.common_slide_data.name.as_deref()
    }

    /// Returns whether the slide is hidden from the slideshow.
    pub fn hidden(&self) -> bool {
        self.record.slide.hidden()
    }

    /// Returns whether the slide carries an explicit `p:bg` element.
    pub fn has_explicit_background(&self) -> bool {
        self.record.slide.has_explicit_background()
    }

    /// Returns the direct `p:bgPr` fill when the slide background has one.
    pub fn background_fill(&self) -> Option<&'a Fill> {
        match self
            .record
            .slide
            .common_slide_data
            .background
            .as_ref()?
            .rendering()
        {
            BackgroundRendering::Properties(fill) => fill.as_deref(),
            BackgroundRendering::Reference { .. } | BackgroundRendering::Unsupported(_) => None,
        }
    }

    /// Returns one immediate z-order child by zero-based index.
    pub fn shape(&self, index: usize) -> Option<ShapeRef<'a>> {
        self.record
            .slide
            .common_slide_data
            .shape_tree
            .children
            .get(index)
            .map(shape_ref)
    }

    /// Returns the first title or centered-title placeholder on the slide.
    pub fn title(&self) -> Option<ShapeRef<'_>> {
        self.shapes().find(|shape| {
            matches!(
                shape_placeholder(shape.child).and_then(|placeholder| placeholder.ph_type.as_ref()),
                Some(PhType::Title | PhType::CenteredTitle)
            )
        })
    }

    /// Returns the placeholder whose OOXML placeholder index equals `index`.
    pub fn placeholder(&self, index: u32) -> Option<ShapeRef<'_>> {
        self.shapes().find(|shape| {
            shape_placeholder(shape.child).map(|placeholder| placeholder.idx.unwrap_or(0))
                == Some(index)
        })
    }

    /// Iterates the immediate shape tree in z-order. [`Shapes::walk`] also
    /// visits the shapes inside groups.
    pub fn shapes(&self) -> Shapes<'a> {
        shapes(&self.record.slide.common_slide_data.shape_tree.children)
    }

    /// Returns visible shape and table text recursively in z-order.
    pub fn text(&self) -> String {
        let mut text = Vec::new();
        collect_text(
            &self.record.slide.common_slide_data.shape_tree.children,
            &mut text,
        );
        text.join("\n")
    }

    /// Compares the accepted notes model, including run distribution and retained XML.
    /// Missing notes compare equal only to missing notes.
    pub fn notes_text_matches(&self, other: &SlideRef<'_>) -> bool {
        self.record.notes.as_ref().map(|record| &record.notes)
            == other.record.notes.as_ref().map(|record| &record.notes)
    }

    /// Returns the one direct writable notes-body text, or no body.
    ///
    /// Grouped, alternate and multiple effective bodies refuse this bounded
    /// frame projection. `notes_text` retains its aggregate-read behavior.
    pub fn notes_text_frame_text(&self) -> Result<Option<String>> {
        let Some(notes) = &self.record.notes else {
            return Ok(None);
        };
        let children = &notes.notes.common_slide_data.shape_tree.children;
        fn effective_bodies(children: &[ShapeTreeChild]) -> usize {
            children.iter().map(|child| {
                let body = usize::from(matches!(child, ShapeTreeChild::Shape(shape)
                    if shape.placeholder.as_ref().is_some_and(|ph| ph.effective_type() == PhType::Body)));
                body + effective_bodies(shape_ref(child).child_slice())
            }).sum()
        }
        let count = effective_bodies(children);
        if count == 0 {
            return Ok(None);
        }
        let direct = children.iter().find_map(|child| match child {
            ShapeTreeChild::Shape(shape)
                if shape
                    .placeholder
                    .as_ref()
                    .is_some_and(|ph| ph.effective_type() == PhType::Body) =>
            {
                Some(shape)
            }
            _ => None,
        });
        match (count, direct) {
            (1, Some(shape)) => Ok(Some(shape.text_body.as_ref().map_or_else(String::new, CT_TextBody::plain_text))),
            _ => Err(Error::InvalidSlideMutation {
                operation: "notes text frame",
                message: "notes_text_frame requires one direct Body placeholder without grouped or alternate bodies; use Slide.notes_text for aggregate reads (its existing setter selects the first writable body)".to_owned(),
            }),
        }
    }

    /// Returns speaker-note text, distinguishing no notes part from empty notes.
    pub fn notes_text(&self) -> Option<String> {
        self.record
            .notes
            .as_ref()
            .map(|notes| notes.notes.notes_text())
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

/// A borrowed shape-tree child.
#[derive(Clone, Copy)]
pub struct ShapeRef<'a> {
    child: &'a ShapeTreeChild,
}

impl PartialEq for ShapeRef<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.child, other.child)
    }
}

impl Eq for ShapeRef<'_> {}

fn shape_ref(child: &ShapeTreeChild) -> ShapeRef<'_> {
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

fn shapes(children: &[ShapeTreeChild]) -> Shapes<'_> {
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

/// A mutably borrowed shape-tree child.
pub struct ShapeMut<'a> {
    owner: ShapeMutOwner<'a>,
}
enum ShapeMutOwner<'a> {
    Presentation {
        presentation: &'a mut Presentation,
        owner: ShapeOwner,
        path: Vec<usize>,
    },
    Child(&'a mut ShapeTreeChild),
}
fn shape_mut(child: &mut ShapeTreeChild) -> ShapeMut<'_> {
    ShapeMut {
        owner: ShapeMutOwner::Child(child),
    }
}
fn owned_shape_mut(
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
    fn set_position_leaf(&mut self, left: Emu, top: Emu) -> Result<()> {
        self.transform_mut("set position")?.offset = Some(CT_Point2D { x: left, y: top });
        Ok(())
    }

    /// Changes the shape extent in EMU.
    fn set_size_leaf(&mut self, width: Emu, height: Emu) -> Result<()> {
        self.transform_mut("set size")?.extent = Some(CT_PositiveSize2D {
            cx: width,
            cy: height,
        });
        Ok(())
    }

    /// Changes the clockwise DrawingML rotation.
    fn set_rotation_leaf(&mut self, rotation: Angle) -> Result<()> {
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
    fn set_adjust_value_leaf(&mut self, name: &str, value: f64) -> Result<()> {
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
    fn set_auto_shape_type_leaf(&mut self, preset: &str) -> Result<()> {
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

    fn transform_mut(&mut self, operation: &'static str) -> Result<&mut CT_Transform2D> {
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
    fn leaf(&self) -> &ShapeTreeChild {
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
    fn leaf_mut(&mut self) -> &mut ShapeTreeChild {
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
    fn into_leaf(self) -> &'a mut ShapeTreeChild {
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
    fn apply_geometry(&mut self, edit: GeometryEdit) -> Result<()> {
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

/// A mutably borrowed ordinary-shape text body.
pub struct TextFrame<'a> {
    body: &'a mut CT_TextBody,
}

/// A borrowed ordinary-shape text body.
#[derive(Clone, Copy, PartialEq)]
pub struct TextFrameRef<'a> {
    body: &'a CT_TextBody,
}

/// A borrowed DrawingML text paragraph.
#[derive(Clone, Copy)]
pub struct TextParagraphRef<'a> {
    paragraph: &'a CT_TextParagraph,
}

/// A borrowed regular DrawingML text run.
#[derive(Clone, Copy)]
pub struct TextRunRef<'a> {
    run: &'a CT_RegularTextRun,
}

/// A borrowed DrawingML table.
#[derive(Clone, Copy)]
pub struct TableRef<'a> {
    table: &'a CT_Table,
}

impl<'a> TableRef<'a> {
    /// Returns the number of explicit table rows.
    pub fn row_count(&self) -> usize {
        self.table.rows.len()
    }

    /// Returns the number of grid columns.
    pub fn column_count(&self) -> usize {
        self.table.grid.columns.len()
    }

    /// Returns one explicit grid cell by zero-based row and column.
    pub fn cell(&self, row: usize, column: usize) -> Option<TableCellRef<'a>> {
        table_cell(self.table, row, column).map(|cell| TableCellRef { cell })
    }

    /// Returns one grid-column width in EMU.
    pub fn column_width(&self, column: usize) -> Option<Emu> {
        self.table.grid.columns.get(column).copied()
    }

    /// Returns one stored row height in EMU.
    pub fn row_height(&self, row: usize) -> Option<Emu> {
        self.table.rows.get(row).map(|row| row.height)
    }

    /// Returns whether first-row table styling is enabled.
    pub fn first_row(&self) -> bool {
        self.table
            .properties
            .as_ref()
            .is_some_and(|properties| properties.first_row)
    }

    /// Returns whether last-row table styling is enabled.
    pub fn last_row(&self) -> bool {
        self.table
            .properties
            .as_ref()
            .is_some_and(|properties| properties.last_row)
    }

    /// Returns whether first-column table styling is enabled.
    pub fn first_column(&self) -> bool {
        self.table
            .properties
            .as_ref()
            .is_some_and(|properties| properties.first_column)
    }

    /// Returns whether last-column table styling is enabled.
    pub fn last_column(&self) -> bool {
        self.table
            .properties
            .as_ref()
            .is_some_and(|properties| properties.last_column)
    }

    /// Returns whether horizontal row banding is enabled.
    pub fn horizontal_banding(&self) -> bool {
        self.table
            .properties
            .as_ref()
            .is_some_and(|properties| properties.band_rows)
    }

    /// Returns whether vertical column banding is enabled.
    pub fn vertical_banding(&self) -> bool {
        self.table
            .properties
            .as_ref()
            .is_some_and(|properties| properties.band_columns)
    }
    /// Returns the retained table style identity without creating properties.
    pub fn style_id(&self) -> Option<&str> {
        self.table.properties.as_ref()?.style_id.as_deref()
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
    fn owner(&self) -> ShapeOwner {
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

/// A behavior-bearing mutable DrawingML table and its containing frame extent.
pub struct TableMut<'a> {
    shape: ShapeMut<'a>,
}

impl<'a> TableMut<'a> {
    /// Returns the number of explicit table rows.
    pub fn row_count(&self) -> usize {
        self.table().rows.len()
    }

    /// Returns the number of grid columns.
    pub fn column_count(&self) -> usize {
        self.table().grid.columns.len()
    }

    /// Returns one explicit grid cell by zero-based row and column.
    pub fn cell(&self, row: usize, column: usize) -> Option<TableCellRef<'_>> {
        table_cell(self.table(), row, column).map(|cell| TableCellRef { cell })
    }

    /// Returns one explicit grid cell for in-place mutation.
    pub fn cell_mut(&mut self, row: usize, column: usize) -> Option<TableCellMut<'_>> {
        table_cell(self.table_mut_raw(), row, column)?;
        Some(TableCellMut {
            table: self.table_mut_raw(),
            row,
            column,
        })
    }

    /// Consumes this handle and returns one explicit cell for mutation.
    pub fn into_cell_mut(self, row: usize, column: usize) -> Option<TableCellMut<'a>> {
        let ShapeTreeChild::GraphicFrame(frame) = self.shape.into_leaf() else {
            return None;
        };
        let table = frame.graphic_data.table_mut()?;
        table_cell(table, row, column)?;
        Some(TableCellMut { table, row, column })
    }

    /// Returns one grid-column width in EMU.
    pub fn column_width(&self, column: usize) -> Option<Emu> {
        self.table().grid.columns.get(column).copied()
    }

    /// Returns the retained table style identity.
    pub fn style_id(&self) -> Option<&str> {
        self.table().properties.as_ref()?.style_id.as_deref()
    }

    /// Changes only the table style reference, preserving grid and source metadata.
    pub fn set_style_id(&mut self, value: Option<String>) -> Result<()> {
        if let Some(value) = value.as_deref() {
            let bytes = value.as_bytes();
            if bytes.len() != 38
                || bytes[0] != b'{'
                || bytes[37] != b'}'
                || !bytes[1..37].iter().enumerate().all(|(index, byte)| {
                    if [8, 13, 18, 23].contains(&index) {
                        *byte == b'-'
                    } else {
                        byte.is_ascii_hexdigit()
                    }
                })
            {
                return Err(invalid_table_mutation(
                    "set table style",
                    "style id must be a braced GUID".to_owned(),
                ));
            }
        }
        if self.style_id() == value.as_deref() {
            return Ok(());
        }
        if let Some(id) = value.as_deref() {
            let canonical = id.to_ascii_uppercase();
            let mut available =
                oxoffice_drawing::table::CT_TableStyle::builtin(&canonical).is_some();
            if let ShapeMutOwner::Presentation { presentation, .. } = &self.shape.owner
                && let Some(part) =
                    import_table_style_part(&presentation.package, &presentation.presentation_part)?
            {
                if presentation.package.content_types.content_type_for(&part)
                    != Some(oxoffice_opc::content_types::TABLE_STYLES)
                {
                    return Err(invalid_table_mutation(
                        "set table style",
                        "table style part has wrong content type".to_owned(),
                    ));
                }
                let styles = import_table_styles(required_part(&presentation.package, &part)?)
                    .map_err(|error| {
                        invalid_table_mutation("set table style", error.to_string())
                    })?;
                let matches = styles
                    .styles
                    .iter()
                    .filter(|(style, _)| style.eq_ignore_ascii_case(id))
                    .count();
                if matches > 1 {
                    return Err(invalid_table_mutation(
                        "set table style",
                        "ambiguous table style definitions".to_owned(),
                    ));
                }
                available |= matches == 1;
            }
            if !available {
                return Err(invalid_table_mutation(
                    "set table style",
                    "style is neither built in nor defined in this presentation".to_owned(),
                ));
            }
        }
        table_properties_mut(self.table_mut_raw()).style_id = value;
        Ok(())
    }

    /// Changes one grid width and synchronizes the containing frame width.
    fn set_column_width_leaf(&mut self, column: usize, width: Emu) -> Result<()> {
        const OPERATION: &str = "set column width";
        if width.0 <= 0 {
            return Err(invalid_table_mutation(
                OPERATION,
                "column width must be positive".to_owned(),
            ));
        }
        if column >= self.table_mut_raw().grid.columns.len() {
            return Err(invalid_table_mutation(
                OPERATION,
                format!("column index {column} is out of range"),
            ));
        }
        let mut staged = self.table_mut_raw().clone();
        staged
            .set_column_width(column, width)
            .map_err(|error| invalid_table_mutation(OPERATION, error.to_string()))?;
        let (total_width, total_height) = table_extent(&staged, OPERATION)?;
        staged
            .to_xml()
            .map_err(|error| invalid_table_mutation(OPERATION, error.to_string()))?;
        *self.table_mut_raw() = staged;
        let height = self
            .transform_mut_raw()
            .extent
            .map_or(total_height, |extent| extent.cy);
        self.transform_mut_raw().extent = Some(CT_PositiveSize2D {
            cx: total_width,
            cy: height,
        });
        Ok(())
    }

    /// Returns one stored row height in EMU.
    pub fn row_height(&self, row: usize) -> Option<Emu> {
        self.table().rows.get(row).map(|row| row.height)
    }

    /// Changes one row height and synchronizes the containing frame height.
    ///
    /// The stored height is a minimum, as in PowerPoint, which grows a row to
    /// fit its text when it lays the table out.
    fn set_row_height_leaf(&mut self, row: usize, height: Emu) -> Result<()> {
        const OPERATION: &str = "set row height";
        if height.0 <= 0 {
            return Err(invalid_table_mutation(
                OPERATION,
                "row height must be positive".to_owned(),
            ));
        }
        if row >= self.table_mut_raw().rows.len() {
            return Err(invalid_table_mutation(
                OPERATION,
                format!("row index {row} is out of range"),
            ));
        }
        let mut staged = self.table_mut_raw().clone();
        staged.rows[row].height = height;
        let (total_width, total_height) = table_extent(&staged, OPERATION)?;
        staged
            .to_xml()
            .map_err(|error| invalid_table_mutation(OPERATION, error.to_string()))?;
        self.table_mut_raw().rows[row].height = height;
        let width = self
            .transform_mut_raw()
            .extent
            .map_or(total_width, |extent| extent.cx);
        self.transform_mut_raw().extent = Some(CT_PositiveSize2D {
            cx: width,
            cy: total_height,
        });
        Ok(())
    }

    /// Inserts a row before `index`, or appends one when `index` equals the
    /// row count, and grows the frame height by the new row's height.
    ///
    /// The new row copies the height of the row above it, or of the first row
    /// when it becomes the first, and each new cell copies that row's cell
    /// properties and first-paragraph formatting without its text. A row
    /// inserted inside a merged cell extends the merge.
    fn insert_row_leaf(&mut self, index: usize) -> Result<()> {
        self.restructure("insert row", |table| table.insert_row(index))
    }

    /// Removes one row, which must not be the only one, and shrinks the frame
    /// height by its height (a frame shorter than its rows first comes up to
    /// them).
    ///
    /// A merged cell across the row loses one row, and when the row holds the
    /// merge origin the row below takes it over with the origin's text.
    fn remove_row_leaf(&mut self, index: usize) -> Result<()> {
        self.restructure("remove row", |table| table.remove_row(index))
    }

    /// Inserts a grid column before `index`, or appends one when `index`
    /// equals the column count, and grows the frame width by its width (a
    /// frame narrower than its grid first comes up to it).
    ///
    /// The new column copies the width of the column left of it, or of the
    /// first column when it becomes the first, and its cells copy that
    /// column's cells as `insert_row` does. A column inserted inside a merged
    /// cell extends the merge.
    fn insert_column_leaf(&mut self, index: usize) -> Result<()> {
        self.restructure("insert column", |table| table.insert_column(index))
    }

    /// Removes one grid column, which must not be the only one, and shrinks
    /// the frame width by its width (a frame narrower than its grid first
    /// comes up to it).
    ///
    /// A merged cell across the column loses one column, and when the column
    /// holds the merge origin the column to its right takes it over with the
    /// origin's text.
    fn remove_column_leaf(&mut self, index: usize) -> Result<()> {
        self.restructure("remove column", |table| table.remove_column(index))
    }

    /// Applies one row or column edit to a staged table and commits it.
    ///
    /// Each frame edge moves by the change of its grid sum, so the frame grows
    /// or shrinks by the inserted or removed row or column. A frame that
    /// PowerPoint measured taller than its stored rows, because it grew rows
    /// to fit their text, keeps that excess; a frame smaller than its grid
    /// first comes up to it.
    fn restructure(
        &mut self,
        operation: &'static str,
        edit: impl FnOnce(&mut CT_Table) -> std::result::Result<(), OxmlError>,
    ) -> Result<()> {
        let (old_width, old_height) = table_extent(self.table_mut_raw(), operation)?;
        let mut staged = self.table_mut_raw().clone();
        edit(&mut staged).map_err(|error| invalid_table_mutation(operation, error.to_string()))?;
        let (new_width, new_height) = table_extent(&staged, operation)?;
        staged
            .to_xml()
            .map_err(|error| invalid_table_mutation(operation, error.to_string()))?;
        let frame = self
            .transform_mut_raw()
            .extent
            .unwrap_or(CT_PositiveSize2D {
                cx: old_width,
                cy: old_height,
            });
        let extent = CT_PositiveSize2D {
            cx: moved_frame_edge(frame.cx, old_width, new_width, operation)?,
            cy: moved_frame_edge(frame.cy, old_height, new_height, operation)?,
        };
        *self.table_mut_raw() = staged;
        self.transform_mut_raw().extent = Some(extent);
        Ok(())
    }

    /// Returns whether first-row table styling is enabled.
    pub fn first_row(&self) -> bool {
        self.table()
            .properties
            .as_ref()
            .is_some_and(|properties| properties.first_row)
    }

    /// Enables or disables first-row table styling.
    pub fn set_first_row(&mut self, value: bool) {
        table_properties_mut(self.table_mut_raw()).first_row = value;
    }

    /// Returns whether last-row table styling is enabled.
    pub fn last_row(&self) -> bool {
        self.table()
            .properties
            .as_ref()
            .is_some_and(|properties| properties.last_row)
    }

    /// Enables or disables last-row table styling.
    pub fn set_last_row(&mut self, value: bool) {
        table_properties_mut(self.table_mut_raw()).last_row = value;
    }

    /// Returns whether first-column table styling is enabled.
    pub fn first_column(&self) -> bool {
        self.table()
            .properties
            .as_ref()
            .is_some_and(|properties| properties.first_column)
    }

    /// Enables or disables first-column table styling.
    pub fn set_first_column(&mut self, value: bool) {
        table_properties_mut(self.table_mut_raw()).first_column = value;
    }

    /// Returns whether last-column table styling is enabled.
    pub fn last_column(&self) -> bool {
        self.table()
            .properties
            .as_ref()
            .is_some_and(|properties| properties.last_column)
    }

    /// Enables or disables last-column table styling.
    pub fn set_last_column(&mut self, value: bool) {
        table_properties_mut(self.table_mut_raw()).last_column = value;
    }

    /// Returns whether horizontal row banding is enabled.
    pub fn horizontal_banding(&self) -> bool {
        self.table()
            .properties
            .as_ref()
            .is_some_and(|properties| properties.band_rows)
    }

    /// Enables or disables horizontal row banding.
    pub fn set_horizontal_banding(&mut self, value: bool) {
        table_properties_mut(self.table_mut_raw()).band_rows = value;
    }

    /// Returns whether vertical column banding is enabled.
    pub fn vertical_banding(&self) -> bool {
        self.table()
            .properties
            .as_ref()
            .is_some_and(|properties| properties.band_columns)
    }

    /// Enables or disables vertical column banding.
    pub fn set_vertical_banding(&mut self, value: bool) {
        table_properties_mut(self.table_mut_raw()).band_columns = value;
    }
    fn table(&self) -> &CT_Table {
        let ShapeTreeChild::GraphicFrame(frame) = self.shape.leaf() else {
            unreachable!("checked table owner")
        };
        match frame.graphic_data.payload() {
            GraphicDataPayload::Table(table) => table,
            _ => unreachable!("checked table payload"),
        }
    }
    fn table_mut_raw(&mut self) -> &mut CT_Table {
        let ShapeTreeChild::GraphicFrame(frame) = self.shape.leaf_mut() else {
            unreachable!("checked table owner")
        };
        frame
            .graphic_data
            .table_mut()
            .expect("checked table payload")
    }
    fn transform_mut_raw(&mut self) -> &mut CT_Transform2D {
        let ShapeTreeChild::GraphicFrame(frame) = self.shape.leaf_mut() else {
            unreachable!("checked table owner")
        };
        &mut frame.transform
    }
    pub fn set_column_width(&mut self, column: usize, width: Emu) -> Result<()> {
        self.shape
            .apply_geometry(GeometryEdit::TableColumn(column, width))
    }
    pub fn set_row_height(&mut self, row: usize, height: Emu) -> Result<()> {
        self.shape
            .apply_geometry(GeometryEdit::TableRow(row, height))
    }
    pub fn insert_row(&mut self, index: usize) -> Result<()> {
        self.shape
            .apply_geometry(GeometryEdit::TableInsertRow(index))
    }
    pub fn remove_row(&mut self, index: usize) -> Result<()> {
        self.shape
            .apply_geometry(GeometryEdit::TableRemoveRow(index))
    }
    pub fn insert_column(&mut self, index: usize) -> Result<()> {
        self.shape
            .apply_geometry(GeometryEdit::TableInsertColumn(index))
    }
    pub fn remove_column(&mut self, index: usize) -> Result<()> {
        self.shape
            .apply_geometry(GeometryEdit::TableRemoveColumn(index))
    }
}

/// One edge of a table cell, drawn by the `a:lnL`, `a:lnR`, `a:lnT`, or `a:lnB`
/// line of its `a:tcPr`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CellBorder {
    Left,
    Right,
    Top,
    Bottom,
}

/// A borrowed explicit table-grid cell.
#[derive(Clone, Copy)]
pub struct TableCellRef<'a> {
    cell: &'a CT_TableCell,
}

impl<'a> TableCellRef<'a> {
    /// Returns only the direct tcPr anchor, without creating properties.
    pub fn vertical_anchor(&self) -> Option<TextAnchor> {
        self.cell
            .properties
            .as_ref()
            .and_then(|properties| properties.anchor)
    }

    /// Returns the cell's visible plain text.
    pub fn text(&self) -> String {
        self.cell
            .text_body
            .as_ref()
            .map_or_else(String::new, CT_TextBody::plain_text)
    }

    /// Returns the cell text body when present.
    pub fn text_frame(&self) -> Option<TextFrameRef<'a>> {
        self.cell
            .text_body
            .as_ref()
            .map(|body| TextFrameRef { body })
    }

    /// Returns whether this cell is the top-left origin of a merge.
    pub fn is_merge_origin(&self) -> bool {
        !self.is_spanned() && (self.cell.row_span > 1 || self.cell.grid_span > 1)
    }

    /// Returns whether this cell continues a merge from another grid cell.
    pub fn is_spanned(&self) -> bool {
        self.cell.horizontal_merge || self.cell.vertical_merge
    }

    /// Returns the cell's own `rowSpan`: the merge height on a merge origin,
    /// one on an unmerged cell, and on a covered cell what its producer wrote
    /// (one, or its origin's).
    pub fn span_height(&self) -> u32 {
        self.cell.row_span
    }

    /// Returns the cell's own `gridSpan`: the merge width on a merge origin,
    /// one on an unmerged cell, and on a covered cell what its producer wrote
    /// (one, or its origin's).
    pub fn span_width(&self) -> u32 {
        self.cell.grid_span
    }

    /// Returns the direct cell fill, when present.
    pub fn fill(&self) -> Option<&Fill> {
        self.cell
            .properties
            .as_ref()
            .and_then(|properties| properties.fill.as_ref())
    }

    /// Returns the optional left, right, top, and bottom cell margins.
    pub fn margins(&self) -> (Option<Emu>, Option<Emu>, Option<Emu>, Option<Emu>) {
        self.cell
            .properties
            .as_ref()
            .map_or((None, None, None, None), |properties| {
                (
                    properties.margin_left,
                    properties.margin_right,
                    properties.margin_top,
                    properties.margin_bottom,
                )
            })
    }

    /// Returns the direct line of one cell edge, when present.
    pub fn border(&self, edge: CellBorder) -> Option<&CT_LineProperties> {
        let properties = self.cell.properties.as_ref()?;
        match edge {
            CellBorder::Left => properties.left.as_ref(),
            CellBorder::Right => properties.right.as_ref(),
            CellBorder::Top => properties.top.as_ref(),
            CellBorder::Bottom => properties.bottom.as_ref(),
        }
    }
}

/// A behavior-bearing mutable table-grid cell addressed within its table.
pub struct TableCellMut<'a> {
    table: &'a mut CT_Table,
    row: usize,
    column: usize,
}

impl<'a> TableCellMut<'a> {
    /// Returns only the direct cell anchor.
    pub fn vertical_anchor(&self) -> Option<TextAnchor> {
        self.cell_ref().vertical_anchor()
    }

    /// Updates ordinary cell formatting without replacing text or its owners.
    /// Clearing an absent anchor never materializes tcPr.
    pub fn set_vertical_anchor(&mut self, anchor: Option<TextAnchor>) {
        if self.vertical_anchor() == anchor {
            return;
        }
        if let Some(properties) = self.cell_mut().properties.as_mut() {
            properties.anchor = anchor;
        } else if anchor.is_some() {
            self.properties_mut().anchor = anchor;
        }
    }

    /// Returns the cell's visible plain text.
    pub fn text(&self) -> String {
        self.cell_ref().text()
    }

    /// Replaces the cell text with one paragraph per line, as
    /// [`CT_TextBody::set_text`] describes.
    pub fn set_text(&mut self, text: &str) {
        self.cell_mut()
            .text_body
            .get_or_insert_with(CT_TextBody::new)
            .set_text(text);
    }

    /// Returns the cell text body for in-place mutation.
    pub fn text_frame(&mut self) -> TextFrame<'_> {
        let body = self
            .cell_mut()
            .text_body
            .get_or_insert_with(CT_TextBody::new);
        TextFrame { body }
    }

    /// Consumes this cell handle and returns its text body for mutation.
    pub fn into_text_frame(self) -> TextFrame<'a> {
        let body = self.table.rows[self.row].cells[self.column]
            .text_body
            .get_or_insert_with(CT_TextBody::new);
        TextFrame { body }
    }

    /// Replaces or clears the direct cell fill.
    pub fn set_fill(&mut self, fill: Option<Fill>) {
        self.properties_mut().fill = fill;
    }

    /// Returns the direct cell fill, when present.
    pub fn fill(&self) -> Option<&Fill> {
        self.cell_ref().cell.properties.as_ref()?.fill.as_ref()
    }

    /// Replaces all four optional cell margins.
    pub fn set_margins(
        &mut self,
        left: Option<Emu>,
        right: Option<Emu>,
        top: Option<Emu>,
        bottom: Option<Emu>,
    ) {
        let properties = self.properties_mut();
        properties.margin_left = left;
        properties.margin_right = right;
        properties.margin_top = top;
        properties.margin_bottom = bottom;
    }

    /// Returns the optional left, right, top, and bottom cell margins.
    pub fn margins(&self) -> (Option<Emu>, Option<Emu>, Option<Emu>, Option<Emu>) {
        let Some(properties) = self.cell_ref().cell.properties.as_ref() else {
            return (None, None, None, None);
        };
        (
            properties.margin_left,
            properties.margin_right,
            properties.margin_top,
            properties.margin_bottom,
        )
    }

    /// Returns the direct line of one cell edge, when present.
    pub fn border(&self, edge: CellBorder) -> Option<&CT_LineProperties> {
        let properties = self.cell_ref().cell.properties.as_ref()?;
        match edge {
            CellBorder::Left => properties.left.as_ref(),
            CellBorder::Right => properties.right.as_ref(),
            CellBorder::Top => properties.top.as_ref(),
            CellBorder::Bottom => properties.bottom.as_ref(),
        }
    }

    /// Replaces or clears the direct line of one cell edge.
    pub fn set_border(&mut self, edge: CellBorder, line: Option<CT_LineProperties>) {
        let properties = self.properties_mut();
        let slot = match edge {
            CellBorder::Left => &mut properties.left,
            CellBorder::Right => &mut properties.right,
            CellBorder::Top => &mut properties.top,
            CellBorder::Bottom => &mut properties.bottom,
        };
        *slot = line;
    }

    /// Returns whether this cell is the top-left origin of a merge.
    pub fn is_merge_origin(&self) -> bool {
        self.cell_ref().is_merge_origin()
    }

    /// Returns whether this cell continues a merge from another grid cell.
    pub fn is_spanned(&self) -> bool {
        self.cell_ref().is_spanned()
    }

    /// Returns the cell's own `rowSpan`: the merge height on a merge origin,
    /// one on an unmerged cell, and on a covered cell what its producer wrote
    /// (one, or its origin's).
    pub fn span_height(&self) -> u32 {
        self.cell_ref().span_height()
    }

    /// Returns the cell's own `gridSpan`: the merge width on a merge origin,
    /// one on an unmerged cell, and on a covered cell what its producer wrote
    /// (one, or its origin's).
    pub fn span_width(&self) -> u32 {
        self.cell_ref().span_width()
    }

    /// Merges the rectangle between this cell and the other grid coordinate.
    pub fn merge_to(&mut self, row: usize, column: usize) -> Result<()> {
        let mut staged = self.table.clone();
        merge_cells(&mut staged, self.row, self.column, row, column)
            .map_err(|message| invalid_table_mutation("merge cells", message))?;
        staged
            .to_xml()
            .map_err(|error| invalid_table_mutation("merge cells", error.to_string()))?;
        *self.table = staged;
        Ok(())
    }

    /// Splits this merge origin back into its explicit grid cells.
    pub fn split(&mut self) -> Result<()> {
        let mut staged = self.table.clone();
        split_cell(&mut staged, self.row, self.column)
            .map_err(|message| invalid_table_mutation("split cell", message))?;
        staged
            .to_xml()
            .map_err(|error| invalid_table_mutation("split cell", error.to_string()))?;
        *self.table = staged;
        Ok(())
    }

    fn cell_ref(&self) -> TableCellRef<'_> {
        TableCellRef {
            cell: table_cell(self.table, self.row, self.column)
                .expect("TableCellMut coordinates were validated at construction"),
        }
    }

    fn cell_mut(&mut self) -> &mut CT_TableCell {
        &mut self.table.rows[self.row].cells[self.column]
    }

    fn properties_mut(&mut self) -> &mut CT_TableCellProperties {
        self.cell_mut()
            .properties
            .get_or_insert_with(CT_TableCellProperties::default)
    }
}

fn table_cell(table: &CT_Table, row: usize, column: usize) -> Option<&CT_TableCell> {
    table.rows.get(row)?.cells.get(column)
}

fn table_properties_mut(table: &mut CT_Table) -> &mut CT_TableProperties {
    table
        .properties
        .get_or_insert_with(CT_TableProperties::default)
}

/// Returns the checked sums of a table's column widths and row heights.
fn table_extent(table: &CT_Table, operation: &'static str) -> Result<(Emu, Emu)> {
    let total_width = table
        .grid
        .columns
        .iter()
        .try_fold(0i64, |total, width| total.checked_add(width.0))
        .ok_or_else(|| {
            invalid_table_mutation(operation, "table width exceeds the EMU range".to_owned())
        })?;
    let total_height = table
        .rows
        .iter()
        .try_fold(0i64, |total, row| total.checked_add(row.height.0))
        .ok_or_else(|| {
            invalid_table_mutation(operation, "table height exceeds the EMU range".to_owned())
        })?;
    if total_width <= 0 || total_height <= 0 {
        return Err(invalid_table_mutation(
            operation,
            "table width and height must remain positive".to_owned(),
        ));
    }
    Ok((Emu(total_width), Emu(total_height)))
}

/// Moves a table frame edge by the grid's change. A frame larger than its
/// grid (rows grown by their text) keeps its margin; one smaller than its
/// grid (Google writes 3,000,000 EMU whatever the grid) is first brought up
/// to the grid, which is what every reader draws.
fn moved_frame_edge(
    frame: Emu,
    old_total: Emu,
    new_total: Emu,
    operation: &'static str,
) -> Result<Emu> {
    new_total
        .0
        .checked_sub(old_total.0)
        .and_then(|change| frame.0.max(old_total.0).checked_add(change))
        .filter(|size| *size > 0)
        .map(Emu)
        .ok_or_else(|| {
            invalid_table_mutation(
                operation,
                "table frame size must remain positive and within the EMU range".to_owned(),
            )
        })
}

fn rectangular_dimensions(table: &CT_Table) -> std::result::Result<(usize, usize), String> {
    let rows = table.rows.len();
    let columns = table.grid.columns.len();
    if rows == 0 || columns == 0 || table.rows.iter().any(|row| row.cells.len() != columns) {
        return Err("table does not have a rectangular explicit cell grid".to_owned());
    }
    Ok((rows, columns))
}

fn merge_cells(
    table: &mut CT_Table,
    first_row: usize,
    first_column: usize,
    second_row: usize,
    second_column: usize,
) -> std::result::Result<(), String> {
    let (rows, columns) = rectangular_dimensions(table)?;
    if first_row >= rows
        || second_row >= rows
        || first_column >= columns
        || second_column >= columns
    {
        return Err("merge coordinate is out of range".to_owned());
    }
    let top = first_row.min(second_row);
    let bottom = first_row.max(second_row);
    let left = first_column.min(second_column);
    let right = first_column.max(second_column);
    if top == bottom && left == right {
        return Err("merge requires at least two cells".to_owned());
    }
    if table.rows[top..=bottom].iter().any(|row| {
        row.cells[left..=right].iter().any(|cell| {
            cell.row_span != 1
                || cell.grid_span != 1
                || cell.horizontal_merge
                || cell.vertical_merge
        })
    }) {
        return Err("merge range contains one or more merged cells".to_owned());
    }
    let row_span = u32::try_from(bottom - top + 1)
        .map_err(|_| "merge row span exceeds the DrawingML range".to_owned())?;
    let grid_span = u32::try_from(right - left + 1)
        .map_err(|_| "merge column span exceeds the DrawingML range".to_owned())?;

    let mut moved_bodies = Vec::new();
    for row in top..=bottom {
        for column in left..=right {
            if row == top && column == left {
                continue;
            }
            let body = table.rows[row].cells[column]
                .text_body
                .take()
                .unwrap_or_default();
            moved_bodies.push((row, column, body));
        }
    }
    for (row, column, mut body) in moved_bodies {
        let origin_body = table.rows[top].cells[left]
            .text_body
            .get_or_insert_with(CT_TextBody::new);
        body.move_content_to(origin_body);
        table.rows[row].cells[column].text_body = Some(body);
    }

    for row in top..=bottom {
        for column in left..=right {
            let cell = &mut table.rows[row].cells[column];
            cell.row_span = if row == top { row_span } else { 1 };
            cell.grid_span = if column == left { grid_span } else { 1 };
            cell.horizontal_merge = column != left;
            cell.vertical_merge = row != top;
        }
    }
    Ok(())
}

fn split_cell(table: &mut CT_Table, row: usize, column: usize) -> std::result::Result<(), String> {
    let (rows, columns) = rectangular_dimensions(table)?;
    let origin = table_cell(table, row, column)
        .ok_or_else(|| "split coordinate is out of range".to_owned())?;
    if origin.horizontal_merge
        || origin.vertical_merge
        || (origin.row_span == 1 && origin.grid_span == 1)
    {
        return Err("only a merge-origin cell can be split".to_owned());
    }
    let bottom = row
        .checked_add(origin.row_span as usize - 1)
        .ok_or_else(|| "merge row span exceeds the table grid".to_owned())?;
    let right = column
        .checked_add(origin.grid_span as usize - 1)
        .ok_or_else(|| "merge column span exceeds the table grid".to_owned())?;
    if bottom >= rows || right >= columns {
        return Err("merge origin span exceeds the table grid".to_owned());
    }
    let row_span = origin.row_span;
    let grid_span = origin.grid_span;
    // The merge flags mark the covered cells. Their own spans are 1, or, in
    // the top row and the left column, repeat the origin's: PowerPoint and
    // python-pptx repeat them, LibreOffice writes 1.
    for current_row in row..=bottom {
        for current_column in column..=right {
            let cell = &table.rows[current_row].cells[current_column];
            let flags = (cell.horizontal_merge, cell.vertical_merge)
                == (current_column != column, current_row != row);
            let row_span_ok =
                cell.row_span == 1 || (current_row == row && cell.row_span == row_span);
            let grid_span_ok =
                cell.grid_span == 1 || (current_column == column && cell.grid_span == grid_span);
            if !(flags && row_span_ok && grid_span_ok) {
                return Err("merge origin does not have a valid continuation rectangle".to_owned());
            }
        }
    }
    for current_row in row..=bottom {
        for current_column in column..=right {
            let cell = &mut table.rows[current_row].cells[current_column];
            cell.row_span = 1;
            cell.grid_span = 1;
            cell.horizontal_merge = false;
            cell.vertical_merge = false;
        }
    }
    Ok(())
}

impl<'a> TextFrame<'a> {
    /// Returns plain text in paragraph and ordered text-choice order.
    pub fn text(&self) -> String {
        self.body.plain_text()
    }

    /// Replaces the frame content with one paragraph per line, as
    /// [`CT_TextBody::set_text`] describes.
    pub fn set_text(&mut self, text: &str) {
        self.body.set_text(text);
    }

    /// Replaces literal text in this text body only and returns the count.
    ///
    /// Matching and formatting are those of
    /// [`Presentation::try_replace_text`]. An empty placeholder is rejected
    /// before anything changes. With `expect`, the replacement runs on a copy
    /// of the body, and a different count leaves the body unchanged and is
    /// returned for the caller to compare. Nothing is serialized, because
    /// replacing run text cannot make a body fail to serialize.
    pub fn try_replace_text(
        &mut self,
        placeholder: &str,
        value: &str,
        expect: Option<usize>,
    ) -> Result<usize> {
        require_replacement_placeholder(placeholder)?;
        let value = &oxoffice_drawing::text::escape_invalid_xml_characters(value);
        let Some(expected) = expect else {
            return Ok(replace_text_in_body(self.body, placeholder, value));
        };
        let mut candidate = self.body.clone();
        let count = replace_text_in_body(&mut candidate, placeholder, value);
        if count == expected {
            *self.body = candidate;
        }
        Ok(count)
    }

    /// Returns the number of paragraphs in the text body.
    pub fn paragraph_count(&self) -> usize {
        self.body.paragraph_count()
    }

    /// Returns one paragraph without mutating the text body.
    pub fn paragraph(&self, index: usize) -> Option<TextParagraphRef<'_>> {
        self.body
            .paragraphs()
            .get(index)
            .map(|paragraph| TextParagraphRef { paragraph })
    }

    /// Returns one paragraph for in-place mutation.
    pub fn paragraph_mut(&mut self, index: usize) -> Option<TextParagraphMut<'_>> {
        self.body
            .paragraph_mut(index)
            .map(|paragraph| TextParagraphMut { paragraph })
    }

    /// Consumes this handle and returns one paragraph for mutation.
    pub fn into_paragraph_mut(self, index: usize) -> Option<TextParagraphMut<'a>> {
        self.body
            .paragraph_mut(index)
            .map(|paragraph| TextParagraphMut { paragraph })
    }

    /// Appends one empty paragraph.
    pub fn add_paragraph(&mut self) -> TextParagraphMut<'_> {
        TextParagraphMut {
            paragraph: self.body.add_paragraph(),
        }
    }

    /// Replaces all four optional text insets.
    ///
    /// An inset outside the 32-bit `ST_Coordinate32` range is rejected before
    /// any inset changes.
    pub fn set_insets(
        &mut self,
        left: Option<Emu>,
        right: Option<Emu>,
        top: Option<Emu>,
        bottom: Option<Emu>,
    ) -> Result<()> {
        let coordinate = |value: Option<Emu>| {
            value
                .map(|value| {
                    i32::try_from(value.0)
                        .map(Coordinate32Value::Emu)
                        .map_err(|_| Error::InvalidShapeMutation {
                            operation: "set text insets",
                            message: format!("{} EMU is outside the coordinate range", value.0),
                        })
                })
                .transpose()
        };
        let (left, right, top, bottom) = (
            coordinate(left)?,
            coordinate(right)?,
            coordinate(top)?,
            coordinate(bottom)?,
        );
        let properties = &mut self.body.body_properties;
        properties.left_inset = left;
        properties.right_inset = right;
        properties.top_inset = top;
        properties.bottom_inset = bottom;
        Ok(())
    }

    /// Sets or clears the direct vertical anchor.
    pub fn set_vertical_anchor(&mut self, anchor: Option<TextAnchor>) {
        self.body.body_properties.anchor = anchor;
    }

    /// Sets or clears the direct wrap choice, `true` wrapping at the frame width.
    pub fn set_word_wrap(&mut self, wrap: Option<bool>) {
        self.body.body_properties.wrap = wrap.map(|wrap| {
            if wrap {
                TextWrap::Square
            } else {
                TextWrap::None
            }
        });
    }

    /// Sets or clears the direct autofit choice.
    ///
    /// Choosing normal autofit again keeps a stored font scale and line
    /// spacing reduction.
    pub fn set_autofit_mode(&mut self, mode: Option<AutofitMode>) {
        let properties = &mut self.body.body_properties;
        properties.autofit = match mode {
            None => None,
            Some(AutofitMode::None) => Some(TextAutofit::NoAutofit),
            Some(AutofitMode::Shape) => Some(TextAutofit::ShapeAutofit),
            Some(AutofitMode::Normal) => match properties.autofit.take() {
                Some(TextAutofit::Normal(normal)) => Some(TextAutofit::Normal(normal)),
                _ => Some(TextAutofit::Normal(NormalAutofit::default())),
            },
        };
    }
}

impl<'a> TextFrameRef<'a> {
    /// Returns plain text in paragraph and ordered text-choice order.
    pub fn text(&self) -> String {
        self.body.plain_text()
    }

    /// Returns the number of paragraphs in the text body.
    pub fn paragraph_count(&self) -> usize {
        self.body.paragraph_count()
    }

    /// Returns one paragraph by zero-based index.
    pub fn paragraph(&self, index: usize) -> Option<TextParagraphRef<'a>> {
        self.body
            .paragraphs()
            .get(index)
            .map(|paragraph| TextParagraphRef { paragraph })
    }

    /// Returns the direct autofit choice on the text body.
    pub fn autofit_mode(&self) -> Option<AutofitMode> {
        self.body
            .body_properties
            .autofit
            .as_ref()
            .map(|autofit| match autofit {
                TextAutofit::NoAutofit => AutofitMode::None,
                TextAutofit::Normal(_) => AutofitMode::Normal,
                TextAutofit::ShapeAutofit => AutofitMode::Shape,
            })
    }

    /// Returns the optional left, right, top, and bottom text insets in EMU.
    ///
    /// A universal measure such as `0.1in` is converted to the nearest EMU.
    pub fn insets(&self) -> (Option<Emu>, Option<Emu>, Option<Emu>, Option<Emu>) {
        let properties = &self.body.body_properties;
        (
            properties.left_inset.as_ref().and_then(coordinate_emu),
            properties.right_inset.as_ref().and_then(coordinate_emu),
            properties.top_inset.as_ref().and_then(coordinate_emu),
            properties.bottom_inset.as_ref().and_then(coordinate_emu),
        )
    }

    /// Returns the direct vertical anchor.
    pub fn vertical_anchor(&self) -> Option<TextAnchor> {
        self.body.body_properties.anchor
    }

    /// Returns the direct wrap choice, `true` when lines wrap at the frame width.
    pub fn word_wrap(&self) -> Option<bool> {
        self.body
            .body_properties
            .wrap
            .map(|wrap| wrap == TextWrap::Square)
    }
}

fn coordinate_emu(value: &Coordinate32Value) -> Option<Emu> {
    match value {
        Coordinate32Value::Emu(value) => Some(Emu(i64::from(*value))),
        Coordinate32Value::UniversalMeasure(value) => {
            let (number, emu_per_unit) = [
                ("mm", 36_000.0),
                ("cm", 360_000.0),
                ("in", 914_400.0),
                ("pt", 12_700.0),
                ("pc", 152_400.0),
                ("pi", 152_400.0),
            ]
            .into_iter()
            .find_map(|(unit, emu_per_unit)| {
                value
                    .strip_suffix(unit)
                    .map(|number| (number, emu_per_unit))
            })?;
            let number = number.parse::<f64>().ok()?;
            Some(Emu((number * emu_per_unit).round() as i64))
        }
    }
}

impl<'a> TextParagraphRef<'a> {
    /// Returns visible text in ordered run, break, and field order.
    pub fn text(&self) -> String {
        self.paragraph
            .runs
            .iter()
            .map(|run| match run {
                TextRun::Run(run) => run.text.value.as_str(),
                TextRun::Break(_) => "\u{000b}",
                TextRun::Field(field) => field.text.as_ref().map_or("", |text| text.value.as_str()),
            })
            .collect()
    }

    /// Returns the direct paragraph level, defaulting to zero.
    pub fn level(&self) -> u8 {
        self.paragraph
            .properties
            .as_ref()
            .and_then(|properties| properties.level)
            .unwrap_or(0)
    }

    /// Returns direct default-run character properties when present.
    pub fn default_run_properties(&self) -> Option<&'a CT_TextCharacterProperties> {
        self.paragraph
            .properties
            .as_ref()
            .and_then(|properties| properties.default_run_properties.as_ref())
    }

    /// Returns direct paragraph properties when present.
    pub fn properties(&self) -> Option<&'a CT_TextParagraphProperties> {
        self.paragraph.properties.as_ref()
    }

    /// Returns the number of regular runs, excluding breaks and fields.
    pub fn run_count(&self) -> usize {
        self.paragraph
            .runs
            .iter()
            .filter(|run| matches!(run, TextRun::Run(_)))
            .count()
    }

    /// Returns one regular run by zero-based regular-run index.
    pub fn run(&self, index: usize) -> Option<TextRunRef<'a>> {
        self.paragraph
            .runs
            .iter()
            .filter_map(|run| match run {
                TextRun::Run(run) => Some(TextRunRef { run }),
                TextRun::Break(_) | TextRun::Field(_) => None,
            })
            .nth(index)
    }
}

impl TextRunRef<'_> {
    /// Returns the run text.
    pub fn text(&self) -> &str {
        &self.run.text.value
    }

    /// Returns direct character properties when present.
    pub fn properties(&self) -> Option<&CT_TextCharacterProperties> {
        self.run.properties.as_ref()
    }

    /// Returns the direct Latin typeface on this run.
    pub fn font_name(&self) -> Option<&str> {
        self.properties()?
            .latin
            .as_ref()
            .map(|font| font.typeface.as_str())
    }

    /// Returns the direct run size in hundredths of a point.
    pub fn font_size(&self) -> Option<i32> {
        self.properties()?.font_size
    }

    /// Returns the direct sRGB run colour as `RRGGBB`.
    pub fn font_color(&self) -> Option<String> {
        let Fill::Solid(fill) = self.properties()?.fill.as_ref()? else {
            return None;
        };
        let ColorChoice::Srgb { value, .. } = fill.color.as_ref()? else {
            return None;
        };
        Some(value.to_string())
    }
}

/// A mutably borrowed DrawingML text paragraph.
pub struct TextParagraphMut<'a> {
    paragraph: &'a mut CT_TextParagraph,
}

impl TextParagraphMut<'_> {
    /// Replaces fields, breaks, and runs with one regular run, split by an
    /// `a:br` at each line feed, CRLF pair or vertical tab, as python-pptx
    /// assigns paragraph text.
    pub fn set_text(&mut self, text: &str) {
        self.paragraph
            .set_text(&text.replace("\r\n", "\u{b}").replace('\n', "\u{b}"));
    }

    /// Appends a regular run after the existing ordered text choices.
    pub fn add_run(&mut self, text: &str) -> TextRunMut<'_> {
        TextRunMut {
            run: self.paragraph.add_run(text),
        }
    }

    /// Returns one regular run for in-place mutation.
    pub fn run_mut(&mut self, index: usize) -> Option<TextRunMut<'_>> {
        self.paragraph
            .runs
            .iter_mut()
            .filter_map(|run| match run {
                TextRun::Run(run) => Some(TextRunMut { run }),
                TextRun::Break(_) | TextRun::Field(_) => None,
            })
            .nth(index)
    }

    /// Sets the direct paragraph level.
    pub fn set_level(&mut self, level: u8) -> bool {
        if level > 8 {
            return false;
        }
        self.paragraph.properties_mut().level = Some(level);
        true
    }

    /// Returns mutable direct default-run character properties.
    pub fn default_run_properties_mut(&mut self) -> &mut CT_TextCharacterProperties {
        self.paragraph
            .properties_mut()
            .default_run_properties
            .get_or_insert_with(CT_TextCharacterProperties::default)
    }

    /// Returns direct paragraph properties when present.
    pub fn properties(&self) -> Option<&CT_TextParagraphProperties> {
        self.paragraph.properties.as_ref()
    }

    /// Replaces the paragraph's direct typed properties.
    pub fn set_properties(&mut self, properties: CT_TextParagraphProperties) {
        *self.paragraph.properties_mut() = properties;
    }

    /// Sets or clears the direct paragraph bullet.
    ///
    /// A preserved picture or follow-text bullet part that the new value
    /// replaces is removed, so the paragraph keeps one bullet choice.
    pub fn set_bullet(&mut self, bullet: Option<TextBullet>) {
        if let Some(properties) = self.paragraph.properties.as_mut() {
            properties.set_bullet(bullet);
        } else if bullet.is_some() {
            self.paragraph.properties_mut().set_bullet(bullet);
        }
    }
}

/// A mutably borrowed regular DrawingML text run.
pub struct TextRunMut<'a> {
    run: &'a mut CT_RegularTextRun,
}

impl TextRunMut<'_> {
    /// Returns direct character properties when present.
    pub fn properties(&self) -> Option<&CT_TextCharacterProperties> {
        self.run.properties.as_ref()
    }

    /// Replaces text while retaining the run's typed and unmodelled state.
    pub fn set_text(&mut self, text: &str) {
        self.run.set_text(text);
    }

    /// Replaces the run's direct typed character properties.
    pub fn set_properties(&mut self, properties: CT_TextCharacterProperties) {
        self.run.properties = Some(properties);
    }

    /// Sets or clears the direct Latin typeface.
    pub fn set_font(&mut self, font: Option<TextFont>) {
        if let Some(properties) = self.run.properties.as_mut() {
            properties.latin = font;
        } else if font.is_some() {
            let mut properties = CT_TextCharacterProperties::default();
            properties.latin = font;
            self.run.properties = Some(properties);
        }
    }
}

fn shape_kind(child: &ShapeTreeChild) -> ShapeKind {
    match child {
        ShapeTreeChild::Shape(_) => ShapeKind::Shape,
        ShapeTreeChild::Picture(_) => ShapeKind::Picture,
        ShapeTreeChild::GraphicFrame(_) => ShapeKind::GraphicFrame,
        ShapeTreeChild::GroupShape(_) => ShapeKind::Group,
        ShapeTreeChild::Connector(_) => ShapeKind::Connector,
        ShapeTreeChild::AlternateContent(_) => ShapeKind::AlternateContent,
    }
}

fn shape_transform(child: &ShapeTreeChild) -> Option<&CT_Transform2D> {
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

fn shape_properties(child: &ShapeTreeChild) -> Option<&CT_ShapeProperties> {
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

    fn child_slice(&self) -> &'a [ShapeTreeChild] {
        match self.child {
            ShapeTreeChild::GroupShape(group) => &group.children,
            ShapeTreeChild::AlternateContent(alternate) => {
                alternate.selected_fallback().unwrap_or_default()
            }
            _ => &[],
        }
    }
}

fn shape_placeholder(child: &ShapeTreeChild) -> Option<&CT_Placeholder> {
    match child {
        ShapeTreeChild::Shape(shape) => shape.placeholder.as_ref(),
        ShapeTreeChild::Picture(picture) => picture.placeholder.as_ref(),
        _ => None,
    }
}

fn collect_text(children: &[ShapeTreeChild], output: &mut Vec<String>) {
    for child in children {
        let shape = shape_ref(child);
        if let Some(text) = shape.text()
            && !text.is_empty()
        {
            output.push(text);
        }
        collect_text(shape.child_slice(), output);
    }
}

/// Writes one notes line and keeps the body's first regular run.
///
/// An empty line stores that run with empty text, so its formatting and
/// unmodelled content stay. A line with text uses the same run. Without one,
/// the general paragraph assignment applies, including its empty-text drop.
fn assign_notes_line(
    paragraph: &mut CT_TextParagraph,
    line: &str,
    formatting: Option<&CT_RegularTextRun>,
) {
    let has_regular_run = paragraph
        .runs
        .iter()
        .any(|run| matches!(run, TextRun::Run(_)));
    if (line.is_empty() || !has_regular_run)
        && let Some(source) = formatting
    {
        let mut run = source.clone();
        run.set_text("");
        if !paragraph.runs.is_empty() {
            paragraph.set_text("");
        }
        paragraph.add_run("");
        if let Some(slot) = paragraph.runs.first_mut() {
            *slot = TextRun::Run(run);
        }
        if line.is_empty() {
            return;
        }
    }
    paragraph.set_text(line);
}

fn first_notes_body_mut(children: &mut [ShapeTreeChild]) -> Option<&mut CT_TextBody> {
    for child in children {
        match child {
            ShapeTreeChild::Shape(shape)
                if shape
                    .placeholder
                    .as_ref()
                    .is_some_and(|placeholder| placeholder.effective_type() == PhType::Body) =>
            {
                return Some(shape.text_body.get_or_insert_with(CT_TextBody::new));
            }
            ShapeTreeChild::GroupShape(group) => {
                if let Some(body) = first_notes_body_mut(&mut group.children) {
                    return Some(body);
                }
            }
            ShapeTreeChild::Shape(_)
            | ShapeTreeChild::Picture(_)
            | ShapeTreeChild::GraphicFrame(_)
            | ShapeTreeChild::Connector(_)
            | ShapeTreeChild::AlternateContent(_) => {}
        }
    }
    None
}

fn require_replacement_placeholder(placeholder: &str) -> Result<()> {
    if placeholder.is_empty() {
        return Err(Error::InvalidSlideMutation {
            operation: "replace text",
            message: "placeholder must not be empty".to_owned(),
        });
    }
    Ok(())
}

fn replace_text_in_slide_record(
    record: &mut SlideRecord,
    placeholder: &str,
    value: &str,
    notes: bool,
) -> usize {
    let slide_count = replace_text_in_children(
        &mut record.slide.common_slide_data.shape_tree.children,
        placeholder,
        value,
    );
    let notes_count = record.notes.as_mut().filter(|_| notes).map_or(0, |notes| {
        replace_text_in_children(
            &mut notes.notes.common_slide_data.shape_tree.children,
            placeholder,
            value,
        )
    });
    slide_count + notes_count
}

/// Whether `replace_text_in_slide_record` would replace anything in a slide
/// or its notes: the same run segments, read without copying the record.
fn slide_record_has_text(record: &SlideRecord, placeholder: &str) -> bool {
    fn in_body(body: &CT_TextBody, placeholder: &str) -> bool {
        body.paragraphs().iter().any(|paragraph| {
            paragraph
                .runs
                .split(|run| !matches!(run, TextRun::Run(_)))
                .any(|segment| {
                    let text = segment
                        .iter()
                        .filter_map(|run| match run {
                            TextRun::Run(run) => Some(run.text.value.as_str()),
                            _ => None,
                        })
                        .collect::<String>();
                    text.contains(placeholder)
                })
        })
    }
    fn in_children(children: &[ShapeTreeChild], placeholder: &str) -> bool {
        children.iter().any(|child| match child {
            ShapeTreeChild::Shape(shape) => shape
                .text_body
                .as_ref()
                .is_some_and(|body| in_body(body, placeholder)),
            ShapeTreeChild::GraphicFrame(frame) => match frame.graphic_data.payload() {
                GraphicDataPayload::Table(table) => table
                    .rows
                    .iter()
                    .flat_map(|row| &row.cells)
                    .filter_map(|cell| cell.text_body.as_ref())
                    .any(|body| in_body(body, placeholder)),
                _ => false,
            },
            ShapeTreeChild::GroupShape(group) => in_children(&group.children, placeholder),
            ShapeTreeChild::Picture(_)
            | ShapeTreeChild::Connector(_)
            | ShapeTreeChild::AlternateContent(_) => false,
        })
    }
    !placeholder.is_empty()
        && (in_children(
            &record.slide.common_slide_data.shape_tree.children,
            placeholder,
        ) || record.notes.as_ref().is_some_and(|notes| {
            in_children(
                &notes.notes.common_slide_data.shape_tree.children,
                placeholder,
            )
        }))
}

fn replace_text_in_children(
    children: &mut [ShapeTreeChild],
    placeholder: &str,
    value: &str,
) -> usize {
    let mut count = 0;
    for child in children {
        match child {
            ShapeTreeChild::Shape(shape) => {
                if let Some(body) = &mut shape.text_body {
                    count += replace_text_in_body(body, placeholder, value);
                }
            }
            ShapeTreeChild::GraphicFrame(frame) => {
                if let Some(table) = frame.graphic_data.table_mut() {
                    for row in &mut table.rows {
                        for cell in &mut row.cells {
                            if let Some(body) = &mut cell.text_body {
                                count += replace_text_in_body(body, placeholder, value);
                            }
                        }
                    }
                }
            }
            ShapeTreeChild::GroupShape(group) => {
                count += replace_text_in_children(&mut group.children, placeholder, value);
            }
            ShapeTreeChild::Picture(_)
            | ShapeTreeChild::Connector(_)
            | ShapeTreeChild::AlternateContent(_) => {}
        }
    }
    count
}

fn replace_text_in_body(body: &mut CT_TextBody, placeholder: &str, value: &str) -> usize {
    let mut count = 0;
    for index in 0..body.paragraph_count() {
        if let Some(paragraph) = body.paragraph_mut(index) {
            count += replace_text_in_paragraph(paragraph, placeholder, value);
        }
    }
    count
}

fn replace_text_in_paragraph(
    paragraph: &mut CT_TextParagraph,
    placeholder: &str,
    value: &str,
) -> usize {
    let mut count = 0;
    let mut segment_start = 0;
    while segment_start < paragraph.runs.len() {
        while segment_start < paragraph.runs.len()
            && !matches!(paragraph.runs[segment_start], TextRun::Run(_))
        {
            segment_start += 1;
        }
        let mut segment_end = segment_start;
        while segment_end < paragraph.runs.len()
            && matches!(paragraph.runs[segment_end], TextRun::Run(_))
        {
            segment_end += 1;
        }
        count += replace_text_in_run_segment(
            &mut paragraph.runs[segment_start..segment_end],
            placeholder,
            value,
        );
        segment_start = segment_end.saturating_add(1);
    }
    count
}

fn replace_text_in_run_segment(runs: &mut [TextRun], placeholder: &str, value: &str) -> usize {
    if placeholder.is_empty() {
        return 0;
    }
    let mut text = String::new();
    let mut ranges = Vec::with_capacity(runs.len());
    for run in runs.iter() {
        let TextRun::Run(run) = run else {
            unreachable!("run segments contain only regular runs")
        };
        let start = text.len();
        text.push_str(&run.text.value);
        ranges.push((start, text.len()));
    }
    let matches = text
        .match_indices(placeholder)
        .map(|(start, _)| (start, start + placeholder.len()))
        .collect::<Vec<_>>();

    for &(match_start, match_end) in matches.iter().rev() {
        let start_run = ranges
            .iter()
            .position(|&(_, end)| match_start < end)
            .expect("non-empty match has a starting run");
        let end_run = ranges
            .iter()
            .position(|&(start, end)| match_end > start && match_end <= end)
            .expect("non-empty match has an ending run");
        let start_offset = match_start - ranges[start_run].0;
        let end_offset = match_end - ranges[end_run].0;

        if start_run == end_run {
            let TextRun::Run(run) = &mut runs[start_run] else {
                unreachable!("run segments contain only regular runs")
            };
            run.text
                .value
                .replace_range(start_offset..end_offset, value);
            continue;
        }

        let TextRun::Run(first) = &mut runs[start_run] else {
            unreachable!("run segments contain only regular runs")
        };
        let first_len = first.text.value.len();
        first
            .text
            .value
            .replace_range(start_offset..first_len, value);
        for run in &mut runs[start_run + 1..end_run] {
            let TextRun::Run(run) = run else {
                unreachable!("run segments contain only regular runs")
            };
            run.set_text("");
        }
        let TextRun::Run(last) = &mut runs[end_run] else {
            unreachable!("run segments contain only regular runs")
        };
        last.text.value.replace_range(..end_offset, "");
    }
    matches.len()
}

#[cfg(feature = "render")]
#[derive(Clone, Copy)]
struct TimelineRequest {
    slide_index: usize,
    outgoing_slide_index: Option<usize>,
    position: TimelinePosition,
    fallback_policy: Option<MediaFallbackPolicy>,
}

#[cfg(feature = "render")]
struct PreparedRenderAssembly {
    first_slide_number: usize,
    input: RenderInput,
    layout: LayoutResult,
    font_manager: FontManager,
    slides: Vec<PreparedSlideAssembly>,
    /// Paragraph directions parallel to `input.slides`, as lowering used them.
    text_directions: Vec<ResolvedSlideTextDirections>,
    size: (f64, f64),
    default_text_style: CT_TextListStyle,
    table_styles: Option<CT_TableStyleList>,
}

fn footer_malformed(part_name: &str, error: impl ToString) -> Error {
    Error::MalformedPart {
        part_name: part_name.to_owned(),
        message: error.to_string(),
    }
}

/// A transition effect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransitionKind {
    Fade,
    Push,
    Wipe,
    Split,
    Cover,
    /// PowerPoint's Uncover, the `p:pull` element.
    Uncover,
    Cut,
    Zoom,
    /// An effect oxoffice reads but does not author, by its element name, such
    /// as `blinds` or `morph`.
    Other(String),
}

impl TransitionKind {
    /// The authorable kinds, by the names [`Self::parse`] accepts.
    pub const NAMES: [&'static str; 8] = [
        "fade", "push", "wipe", "split", "cover", "uncover", "cut", "zoom",
    ];

    /// Parses an authorable kind name, `None` for any other name.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "fade" => Self::Fade,
            "push" => Self::Push,
            "wipe" => Self::Wipe,
            "split" => Self::Split,
            "cover" => Self::Cover,
            "uncover" => Self::Uncover,
            "cut" => Self::Cut,
            "zoom" => Self::Zoom,
            _ => return None,
        })
    }

    /// The kind's name, the element name for [`Self::Other`].
    pub fn name(&self) -> &str {
        match self {
            Self::Fade => "fade",
            Self::Push => "push",
            Self::Wipe => "wipe",
            Self::Split => "split",
            Self::Cover => "cover",
            Self::Uncover => "uncover",
            Self::Cut => "cut",
            Self::Zoom => "zoom",
            Self::Other(name) => name,
        }
    }

    /// The directions this kind accepts, the first one being the default.
    pub fn directions(&self) -> &'static [TransitionDirection] {
        use TransitionDirection::*;
        match self {
            Self::Push | Self::Wipe => &[Left, Up, Right, Down],
            Self::Cover | Self::Uncover => {
                &[Left, Up, Right, Down, LeftUp, RightUp, LeftDown, RightDown]
            }
            Self::Split => &[HorizontalOut, HorizontalIn, VerticalOut, VerticalIn],
            Self::Zoom => &[Out, In],
            Self::Fade | Self::Cut | Self::Other(_) => &[],
        }
    }

    fn element(&self) -> &str {
        match self {
            Self::Uncover => "pull",
            other => other.name(),
        }
    }
}

/// The direction an effect moves the incoming slide, as the effect's `dir`
/// and, for split, `orient` attributes write it. `Left` is `dir="l"`, which
/// PowerPoint's user interface calls "From Right" for push and wipe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransitionDirection {
    Left,
    Up,
    Right,
    Down,
    LeftUp,
    RightUp,
    LeftDown,
    RightDown,
    In,
    Out,
    HorizontalIn,
    HorizontalOut,
    VerticalIn,
    VerticalOut,
}

impl TransitionDirection {
    /// Every direction, by the names [`Self::parse`] accepts.
    pub const NAMES: [&'static str; 14] = [
        "left",
        "up",
        "right",
        "down",
        "left-up",
        "right-up",
        "left-down",
        "right-down",
        "in",
        "out",
        "horizontal-in",
        "horizontal-out",
        "vertical-in",
        "vertical-out",
    ];

    const ALL: [Self; 14] = [
        Self::Left,
        Self::Up,
        Self::Right,
        Self::Down,
        Self::LeftUp,
        Self::RightUp,
        Self::LeftDown,
        Self::RightDown,
        Self::In,
        Self::Out,
        Self::HorizontalIn,
        Self::HorizontalOut,
        Self::VerticalIn,
        Self::VerticalOut,
    ];

    /// Parses a direction name such as `left` or `horizontal-in`.
    pub fn parse(name: &str) -> Option<Self> {
        Self::NAMES
            .iter()
            .position(|candidate| *candidate == name)
            .map(|index| Self::ALL[index])
    }

    /// The direction's name.
    pub fn name(self) -> &'static str {
        let index = Self::ALL
            .iter()
            .position(|candidate| *candidate == self)
            .expect("every direction is listed");
        Self::NAMES[index]
    }

    fn attributes(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Left => &[("dir", "l")],
            Self::Up => &[("dir", "u")],
            Self::Right => &[("dir", "r")],
            Self::Down => &[("dir", "d")],
            Self::LeftUp => &[("dir", "lu")],
            Self::RightUp => &[("dir", "ru")],
            Self::LeftDown => &[("dir", "ld")],
            Self::RightDown => &[("dir", "rd")],
            Self::In => &[("dir", "in")],
            Self::Out => &[("dir", "out")],
            Self::HorizontalIn => &[("orient", "horz"), ("dir", "in")],
            Self::HorizontalOut => &[("orient", "horz"), ("dir", "out")],
            Self::VerticalIn => &[("orient", "vert"), ("dir", "in")],
            Self::VerticalOut => &[("orient", "vert"), ("dir", "out")],
        }
    }
}

/// A slide transition and how the slide show advances past the slide.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SlideTransition {
    /// The effect, `None` for a transition that only sets advance timing.
    pub kind: Option<TransitionKind>,
    /// The effect direction, `None` for the kind's default.
    pub direction: Option<TransitionDirection>,
    /// The effect duration in milliseconds, `None` for the default speed.
    pub duration_ms: Option<u64>,
    /// Whether a click advances, true by default.
    pub advance_on_click: Option<bool>,
    /// Advance automatically after this many milliseconds.
    pub advance_after_ms: Option<u64>,
}

impl SlideTransition {
    fn from_model(model: &oxoffice_pptx_oxml::timing::CT_SlideTransition) -> Self {
        let kind = model.effect.as_ref().map(|effect| match effect {
            oxoffice_pptx_oxml::timing::TransitionEffect::Cut => TransitionKind::Cut,
            oxoffice_pptx_oxml::timing::TransitionEffect::Fade => TransitionKind::Fade,
            oxoffice_pptx_oxml::timing::TransitionEffect::Wipe => TransitionKind::Wipe,
            oxoffice_pptx_oxml::timing::TransitionEffect::Push => TransitionKind::Push,
            oxoffice_pptx_oxml::timing::TransitionEffect::Zoom => TransitionKind::Zoom,
            oxoffice_pptx_oxml::timing::TransitionEffect::Split => TransitionKind::Split,
            oxoffice_pptx_oxml::timing::TransitionEffect::Cover => TransitionKind::Cover,
            oxoffice_pptx_oxml::timing::TransitionEffect::Uncover => TransitionKind::Uncover,
            oxoffice_pptx_oxml::timing::TransitionEffect::Morph => {
                TransitionKind::Other("morph".to_owned())
            }
            oxoffice_pptx_oxml::timing::TransitionEffect::Other(name) => match name.as_str() {
                "split" => TransitionKind::Split,
                "cover" => TransitionKind::Cover,
                "pull" => TransitionKind::Uncover,
                _ => TransitionKind::Other(name.clone()),
            },
        });
        let parameter = |name: &str| {
            model
                .effect_parameters
                .iter()
                .find(|parameter| parameter.name == name)
                .map(|parameter| parameter.value.as_str())
        };
        // An effect that names any direction attribute reads with the
        // schema defaults for the others, so `<p:split orient="vert"/>` is
        // vertical-out. One that names none keeps the default, `None`.
        let direction = kind.as_ref().and_then(|kind| {
            let defaults = kind.directions().first()?.attributes();
            if !defaults.iter().any(|(name, _)| parameter(name).is_some()) {
                return None;
            }
            kind.directions().iter().copied().find(|direction| {
                direction.attributes().iter().all(|(name, value)| {
                    let default = defaults
                        .iter()
                        .find(|(default_name, _)| default_name == name)
                        .map(|(_, value)| *value);
                    parameter(name).or(default) == Some(*value)
                })
            })
        });
        Self {
            kind,
            direction,
            duration_ms: model.duration_ms,
            advance_on_click: model.advance_on_click,
            advance_after_ms: model.advance_after_ms,
        }
    }

    /// Builds the `p:transition` this value writes, `None` when it has no
    /// effect and the default advance, which is no transition at all.
    fn to_model(
        &self,
        previous: Option<&oxoffice_pptx_oxml::timing::CT_SlideTransition>,
    ) -> Result<Option<oxoffice_pptx_oxml::timing::CT_SlideTransition>> {
        let operation = "set a slide transition";
        if let Some(TransitionKind::Other(name)) = &self.kind {
            return Err(invalid_slide_mutation(
                operation,
                format!(
                    "transition {name:?} cannot be authored, use one of {}",
                    TransitionKind::NAMES.join(", ")
                ),
            ));
        }
        if let Some(direction) = self.direction {
            let accepted = self
                .kind
                .as_ref()
                .map_or(&[][..], TransitionKind::directions);
            if !accepted.contains(&direction) {
                let kind = self.kind.as_ref().map_or("no effect", TransitionKind::name);
                let names = accepted
                    .iter()
                    .map(|direction| direction.name())
                    .collect::<Vec<_>>();
                return Err(invalid_slide_mutation(
                    operation,
                    if names.is_empty() {
                        format!("{kind} takes no direction")
                    } else {
                        format!(
                            "{kind} does not take direction {}, use one of {}",
                            direction.name(),
                            names.join(", ")
                        )
                    },
                ));
            }
        }
        if self.kind.is_none() && self.duration_ms.is_some() {
            return Err(invalid_slide_mutation(
                operation,
                "a duration needs an effect, set a transition type first",
            ));
        }
        if self.kind.is_none() && self.advance_on_click.is_none() && self.advance_after_ms.is_none()
        {
            return Ok(None);
        }
        let attributes = self
            .direction
            .map_or(&[][..], TransitionDirection::attributes);
        let effect = self.kind.as_ref().map(|kind| (kind.element(), attributes));
        match previous {
            Some(previous) => previous.with_settings(
                effect,
                self.duration_ms,
                self.advance_on_click,
                self.advance_after_ms,
            ),
            None => oxoffice_pptx_oxml::timing::CT_SlideTransition::authored(
                effect,
                self.duration_ms,
                self.advance_on_click,
                self.advance_after_ms,
            ),
        }
        .map(Some)
        .map_err(|error| invalid_slide_mutation(operation, error.to_string()))
    }
}

impl SlideRef<'_> {
    /// Returns the selected transition and optional advance settings.
    pub fn transition(&self) -> Option<SlideTransition> {
        self.record
            .slide
            .transition
            .as_ref()
            .map(SlideTransition::from_model)
    }
}

impl SlideMut<'_> {
    /// Changes owned transition settings while retaining producer siblings.
    pub fn set_transition(&mut self, transition: Option<&SlideTransition>) -> Result<bool> {
        self.presentation
            .set_slide_transition(self.index, transition)
    }
}

impl Presentation {
    /// Stages a selected transition change and validates the resulting package.
    pub fn set_slide_transition(
        &mut self,
        index: usize,
        transition: Option<&SlideTransition>,
    ) -> Result<bool> {
        let mut candidate = self.clone();
        let record = candidate.slides.get_mut(index).ok_or_else(|| {
            invalid_slide_mutation("set a slide transition", "slide index out of range")
        })?;
        let model = transition
            .map(|value| value.to_model(record.slide.transition.as_ref()))
            .transpose()?
            .flatten();
        let changed = record.slide.transition != model;
        if changed {
            Arc::make_mut(record).slide.transition = model;
        }
        if changed {
            self.commit_candidate(candidate)?;
        }
        Ok(changed)
    }
    /// Stages one transition intent against each slide's retained source.
    pub fn set_all_transitions(&mut self, transition: Option<&SlideTransition>) -> Result<bool> {
        let mut candidate = self.clone();
        let mut changed = false;
        for record in &mut candidate.slides {
            let model = transition
                .map(|value| value.to_model(record.slide.transition.as_ref()))
                .transpose()?
                .flatten();
            if model != record.slide.transition {
                Arc::make_mut(record).slide.transition = model;
                changed = true;
            }
        }
        if changed {
            self.commit_candidate(candidate)?;
        }
        Ok(changed)
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

fn checked_geometry_emu(value: f64) -> Result<Emu> {
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
enum Members<'t> {
    Tree(&'t mut CT_ShapeTree),
    Group(&'t mut CT_GroupShape),
}

impl<'t> Members<'t> {
    fn at(tree: &'t mut CT_ShapeTree, group: &[usize]) -> Option<Self> {
        if group.is_empty() {
            Some(Self::Tree(tree))
        } else {
            group_at_mut(&mut tree.children, group).map(Self::Group)
        }
    }

    fn children(&self) -> &[ShapeTreeChild] {
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
fn path_of_id(children: &[ShapeTreeChild], shape_id: u32) -> Option<Vec<usize>> {
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

fn tree_has_connector(children: &[ShapeTreeChild]) -> bool {
    children.iter().any(|child| match child {
        ShapeTreeChild::Connector(_) => true,
        ShapeTreeChild::GroupShape(group) => tree_has_connector(&group.children),
        _ => false,
    })
}

fn malformed_shape_child(part_name: &str, error: impl ToString) -> Error {
    Error::MalformedPart {
        part_name: part_name.to_owned(),
        message: error.to_string(),
    }
}

fn validate_shape_child(child: &ShapeTreeChild, part_name: &str) -> Result<()> {
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

fn child_at<'t>(children: &'t [ShapeTreeChild], path: &[usize]) -> Option<&'t ShapeTreeChild> {
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

fn child_at_mut<'t>(
    children: &'t mut [ShapeTreeChild],
    path: &[usize],
) -> Option<&'t mut ShapeTreeChild> {
    let (last, groups) = path.split_last()?;
    if groups.is_empty() {
        return children.get_mut(*last);
    }
    group_at_mut(children, groups)?.children.get_mut(*last)
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
    fn refresh_connector_attachments(
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
    fn edit_shape_geometry(
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
enum GeometryEdit {
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

fn apply_leaf_geometry(child: &mut ShapeTreeChild, edit: GeometryEdit) -> Result<()> {
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

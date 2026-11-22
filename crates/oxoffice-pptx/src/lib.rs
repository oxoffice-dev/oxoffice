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
mod shared_parts;
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

mod transitions;

pub use transitions::{SlideTransition, TransitionDirection, TransitionKind};

mod slides;

use slides::{slide_mut, slide_ref};

mod arrange;

pub(crate) use arrange::connector_endpoints;
pub use arrange::{ArrangeReference, ConnectorEnd, DistributeDirection, ShapeAlignment};
use arrange::{
    GeometryEdit, Members, apply_leaf_geometry, checked_geometry_emu, path_of_id,
    tree_has_connector,
};

mod shapes;
mod tables;
mod text;

pub use shapes::{
    AutofitMode, ConnectorType, FillOwnerLocation, ShapeKind, ShapeType, ShapeWalk, Shapes,
};
use shapes::{
    ShapeMutOwner, append_member, append_new_member, child_at, child_at_mut, connector_member,
    detach_connectors, fit_group_to_members, group_at_mut, group_member,
    invalid_shape_construction, member_box, owned_shape_mut, positioned_transform, preset_member,
    shape_id_count, shape_kind, shape_mut, shape_placeholder, shape_properties, shape_ref,
    shape_transform, shapes, table_member, textbox_member, validate_shape_child,
};
pub use tables::{CellBorder, TableCellMut, TableCellRef};
use tables::{invalid_table_mutation, table_properties_mut};
use text::collect_text;
pub use text::{
    ReplacementCountMismatch, TextParagraphMut, TextParagraphRef, TextRunMut, TextRunRef,
};

mod comments;

mod hyperlinks;

#[cfg(any(feature = "default-template", feature = "render"))]
const DEFAULT_TEMPLATE: &[u8] = include_bytes!("../assets/default.pptx");

/// A result returned by the `oxoffice-pptx` read facade.
pub type Result<T> = std::result::Result<T, Error>;

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

/// A mutably borrowed slide in presentation order.
pub struct SlideMut<'a> {
    presentation: &'a mut Presentation,
    index: usize,
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

/// A borrowed shape-tree child.
#[derive(Clone, Copy)]
pub struct ShapeRef<'a> {
    child: &'a ShapeTreeChild,
}

/// A mutably borrowed shape-tree child.
pub struct ShapeMut<'a> {
    owner: ShapeMutOwner<'a>,
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

/// A borrowed DrawingML table.
#[derive(Clone, Copy)]
pub struct TableRef<'a> {
    table: &'a CT_Table,
}

/// A behavior-bearing mutable DrawingML table and its containing frame extent.
pub struct TableMut<'a> {
    shape: ShapeMut<'a>,
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

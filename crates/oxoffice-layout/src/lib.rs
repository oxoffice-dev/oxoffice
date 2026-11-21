//! Format-neutral layout output and font primitives.

pub mod bundled_fonts;
pub mod error;
pub mod font;
pub mod glyph_coverage;
pub mod line;
pub mod output;
pub mod paint;
pub mod path;
pub mod transform;

pub use error::{LayoutError, Result};
pub use font::{
    FontContribution, FontContributionScope, FontFile, FontManager, FontMetrics, FontTraceEvent,
    GlyphCluster, MultilingualTextSegment, ShapedText, TextDirection, TextScript,
};
pub use glyph_coverage::missing_glyph_diagnostics;
pub use line::{
    Align, ForcedBreakKind, FormMark, InlineItem, LayoutLine, LineBreakParams, LineItem,
    LineSpacing, NoteRef, NoteStream, TabAlign, TabLeader, TabStop, TextSegment, Underline,
    break_into_lines, break_multilingual_into_lines,
};
pub use output::{
    ChoiceOption, Color, Diagnostic, DocumentMetadata, DocumentStructure, Effect, FieldAlignment,
    FieldKind, FieldSource, FontData, FontId, FormField, FormFieldFragment, FormFieldId,
    FormFieldKind, FormMarker, GlyphRun, GroupElement, LayoutResult, MediaId, MultilingualGlyphRun,
    OutlineEntry, PageFrame, PathElement, Point, PositionedElement, Rect, SourceNodeId, SourceSpan,
    StructureId, StructureNode, StructureRole, TabAlignedField, walk,
};
pub use paint::{GradientStop, LineCap, LineJoin, Paint, Stroke};
pub use path::{FillRule, Path, PathCommand};
pub use transform::Transform;

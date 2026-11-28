//! High-level API for reading, writing, and converting DOCX documents.
//!
//! Provides a Python-docx-like interface for working with Word documents.
//!
//! # Examples
//!
//! ```no_run
//! use oxoffice_docx::Document;
//!
//! // Create a new document
//! let mut doc = Document::new();
//! doc.add_paragraph("Hello, World!");
//! doc.save("output.docx").unwrap();
//!
//! // Open an existing document
//! let doc = Document::open("existing.docx").unwrap();
//! for para in doc.paragraphs() {
//!     println!("{}", para.text());
//! }
//! ```

#![allow(clippy::too_many_arguments)]

mod bibliography;
mod building_block;
mod comments;
mod comparison;
mod content_control;
mod document;
mod embedded;
mod epub;
mod error;
mod field;
mod flat_opc;
mod html;
mod math;
mod odt;
pub mod paragraph;
mod redaction;
mod revision;
mod rtf;
pub mod run;
pub mod style;
mod svg;
pub use svg::render_page;
pub mod table;
mod template;
pub use template::{TEMPLATE_GRAMMAR_VERSION, TemplateFormatContext};

pub use bibliography::{
    BibliographyAuthor, BibliographyContributor, BibliographyContributorRole, BibliographyOptions,
    BibliographyPerson, BibliographyProperty, BibliographySource, BibliographySourceField,
    BibliographySourceInfo, BibliographySourceKind, BibliographyStyle, BibliographyStyleInfo,
    BibliographyUpdateReport, CitationOptions, CitationSourceOptions,
};
pub use building_block::{BuildingBlock, BuildingBlockInfo, BuildingBlockKind};
pub use comments::{
    BookmarkRef, CommentDate, CommentMetadata, CommentRef, RevisionDate, RunPosition, RunRange,
    StoryRangeKind, StoryRangeRef, StoryRunPosition, StoryRunRange, TextCommentOptions,
};
pub use comparison::{
    ComparisonDiagnostic, ComparisonGranularity, ComparisonOptions, ComparisonStoryKind,
};
pub use content_control::{
    ContentControlAppearance, ContentControlEdit, ContentControlLock, ContentControlPlacement,
    ContentControlRef, ContentControlSpec, ContentControlTypePayload, ControlBlock, FormEdit,
    FormFieldAssignment, FormFieldRule, FormFieldTarget, FormFieldValue, FormResetValue,
    FormValidationIssue,
};
pub use document::{
    AccessibilityIssue, BodyBlocksChange, BodyContentRef, BodyItemContext, BodyItemOwners,
    BodyItemRef, ContentFragment, ContentLocation, ContentMeasurement, Document, DocumentFragment,
    DrawingHorizontalAlignment, DrawingHorizontalRelativeFrom, DrawingVerticalAlignment,
    DrawingVerticalRelativeFrom, DrawingWrap, EditReport, EditUndo, EditUnit, ElementExtent,
    EmbeddedFont, EmbeddedFontKind, FontDefinition, FontEmbeddingLicense, FragmentConflictPolicy,
    HeaderFooterKind, ImageInfo, ImageWatermarkOptions, IssueSeverity, LinkInfo, ListItemKind,
    ListLevel, ListLevelRestart, ListLevelSuffix, ListNumberFormat, NoteFamily, NoteNumberFormat,
    NotePlacement, NotePolicy, NoteRestart, NoteSpecialRecord, NumberedOutlineNode,
    NumberingDefinition, NumberingDefinitionLevel, NumberingFormat, NumberingInstance,
    NumberingJournal, NumberingLevel, NumberingLevelOverride, OutlineNode, OwnerElementXml,
    PictureAnchor, PictureCrop, PictureOccurrence, PictureOptions, RemovedBodyBlocks,
    RenderOptions, ReplaceOptions, ReplacementCountMismatch, ReplacementRefusal, Section,
    SectionRef, SectionStory, SlotHit, StoryError, StoryId, StoryItemKind, StoryItemRef,
    StoryItemSnapshot, StoryKind, StoryOwnerSnapshots, StoryParagraph, StoryText, StoryTextItem,
    TextBoxDirection, TextBoxOptions, TextDiagnostic, TextReport, TextWatermarkOptions,
    TrackedChanges, TrackedDocument, TrackedSession, UncheckedReplacement, UnreachedReplacement,
    UnsupportedXmlRef, ValidationFinding, ValidationReport, ValidationSeverity,
    WordCreationProfile, WordPackageClass, derive_font_key, resolved_xml_events,
    validate_document_bytes, validate_document_file,
};
pub use embedded::{
    EmbeddedContentInfo, EmbeddedContentKind, EmbeddedMutationPolicy, EmbeddedSignatureState,
};
pub use epub::{EpubDiagnostic, EpubWriteResult};
pub use error::{Error, Result};
pub use field::{
    AuthorityEntry, BarcodeCaseStyle, BarcodeField, BarcodeKind, BarcodePointOfSaleStyle,
    CaptionOptions, CaptionTarget, CrossReferenceNumber, CrossReferenceOptions, Field,
    FieldDateTime, FieldEvaluation, FieldEvaluationContext, FieldOutcome, FieldsUpdateReport,
    GeneratedTablesReport, IndexEntry, IndexOptions, LayoutBackedFieldUpdateReport,
    LegacyCheckBoxSize, LegacyFormFieldDefinition, LegacyFormFieldInfo, LegacyFormFieldKind,
    LegacyFormFieldOptions, LegacyFormFieldValue, LegacyFormInfoText, LegacyTextInputType,
    MailMergeControl, MailMergeData, MailMergeFieldMapping, MailMergeFormatContext,
    MailMergeFormattedText, MailMergeImage, MailMergePackageDiagnostic,
    MailMergePackageDiagnosticKind, MailMergePackageOptions, MailMergePackageSource,
    MailMergePreparedData, MailMergeRecipient, MailMergeRecord, MailMergeSourcePolicy,
    MailMergeTableBudget, MailMergeValue, SequenceOptions, TableOfAuthoritiesOptions,
    TableOfFiguresOptions, TcField, TocEntrySelection, TocField, TocRebuildReport,
    validate_mail_merge_external_uri,
};
pub use html::{
    HtmlDiagnostic, HtmlFragmentInsertResult, HtmlImageResource, HtmlReadResult, MhtmlDiagnostic,
    MhtmlReadResult, MhtmlWriteResult,
};
pub use math::{
    MathConversionDiagnostic, MathConversionResult, equation_from_latex, equation_from_mathml,
    equation_to_latex, equation_to_mathml,
};
pub use odt::{OdtDiagnostic, OdtReadResult, OdtWriteResult};
pub use oxoffice_chart::{ChartData, ChartKind, LegendPosition, LegendSettings, RgbColor};
pub use oxoffice_core::app_properties::AppProperties;
pub use oxoffice_core::core_properties::CoreProperties;
pub use oxoffice_core::custom_properties::{CustomProperty, CustomPropertyValue};
pub use oxoffice_core::{HalfPoint, Length, Twips};
pub use oxoffice_docx_layout::{RevisionStyle, RevisionView, WordLayoutResult};
pub use oxoffice_docx_oxml::document::{CT_DocGrid, ST_DocGrid};
pub use oxoffice_docx_oxml::header_footer::HdrFtrType;
pub use oxoffice_docx_oxml::math::{
    CT_OMath, CT_OMathPara, FractionType, LimitLocation, MathAccent, MathArgument, MathBar,
    MathBorderBox, MathBox, MathDelimiter, MathEquationArray, MathExpression, MathFraction,
    MathFunction, MathGroupCharacter, MathJustification, MathLimit, MathMatrix,
    MathMatrixProperties, MathMatrixRow, MathNary, MathParagraphProperties, MathPhantom,
    MathPreSubSuperscript, MathProperties, MathRadical, MathRun, MathRunProperties, MathScript,
    MathScriptStyle, MathStyle, MathSubSuperscript, MathVerticalPosition, MatrixBaseJustification,
    MatrixColumnJustification, OfficeMath,
};
pub use oxoffice_docx_oxml::properties::{
    CT_EastAsianLayout, CT_FitText, CT_PPr, CT_RPr, ST_Em, ST_TextEffect,
};
pub use oxoffice_docx_oxml::ruby::{CT_Ruby, CT_RubyPr, CT_RubyRun, ST_RubyAlign};
pub use oxoffice_docx_oxml::settings::{
    CharacterSpacingControl, CompatibilityOption, CompatibilitySetting, CryptAlgorithmClass,
    CryptAlgorithmType, CryptProviderType, DocumentProofState, DocumentProtection, DocumentView,
    DocumentZoom, MailMerge, MailMergeDestination, MailMergeDocumentType, ProofState,
    ProtectionMode, SettingsDiagnostic, SettingsDiagnosticReason, ThemeFontLanguage, ZoomKind,
};
pub use oxoffice_docx_oxml::styles::{StyleType, TableStyleRegion};
pub use oxoffice_docx_oxml::text::{
    AcceptedRunPath, AcceptedRunPathSegment, ST_PTabAlignment, ST_PTabLeader, ST_PTabRelativeTo,
    SpecialCharacter,
};
pub use oxoffice_drawing::theme::CT_OfficeStyleSheet;
#[cfg(feature = "digital-signatures")]
pub use oxoffice_opc::{
    CoveredRelationship, SignatureIssue, SignatureReport, SignerCertificateIdentity,
};
pub use oxoffice_opc::{PackageReadLimits, SaveOptions};
pub use oxoffice_pdf::{PdfConformance, RasterFormat, RasterOptions, RasterOutput};
pub use paragraph::{
    Alignment, BorderStyle, DropCap, FrameAnchor, FrameWrap, HyperlinkItemRef, HyperlinkRef,
    Paragraph, ParagraphBorderEdge, ParagraphBorderRef, ParagraphFrame, ParagraphItemRef,
    ParagraphMark, ParagraphMarkRef, ParagraphRef, ParagraphTextAlignment, ParagraphTextDirection,
    SectionBreak, TabAlignment, TabLeader, TabStopRef, TextSpan, TextboxTightWrap,
    auto_line_spacing,
};
pub use redaction::RedactionReport;
pub use revision::{RevisionKind, RevisionRef, StoryRevision};
pub use rtf::{RtfDiagnostic, RtfReadResult, RtfWriteResult};
pub use run::{
    BreakKind, DrawingKind, DrawingRef, DrawingRelationshipKind, FieldDisplaySegmentRef, FieldKind,
    FieldRef, LegacyHorizontalRuleRef, Run, RunFontSlot, RunItemRef, RunProperties, RunRef,
    UnderlineStyle, underline_from_code,
};
pub use style::{ConditionalTableStyle, Style, StyleBuilder};
pub use svg::{SvgDiagnostic, SvgRenderResult};
pub use table::{
    Cell, CellBorderEdge, CellItemRef, CellRef, CellTextDirection, Row, RowHeight, RowRef, Table,
    TableAnchor, TableBorderEdge, TableBorderRef, TableCellMargins, TableConditionalFormatting,
    TableFloatPosition, TableFloatX, TableFloatY, TableLayout, TableLook, TableOverlap, TableRef,
    TableTextDistance, TableWidth, VMerge, VerticalAlignment,
};

//! Section properties (`w:sectPr`) and the helpers that read and write them.

use super::*;

#[derive(Clone, Copy)]
pub(super) enum SectionScalar {
    Measurement,
    Count,
}
impl SectionScalar {
    pub(super) fn for_names(element: &[u8], attribute: &[u8]) -> Option<Self> {
        match (element, attribute) {
            (b"pgSz", b"w" | b"h")
            | (
                b"pgMar",
                b"top" | b"right" | b"end" | b"bottom" | b"left" | b"start" | b"gutter" | b"header"
                | b"footer",
            )
            | (b"cols", b"space")
            | (b"col", b"w" | b"space") => Some(Self::Measurement),
            (b"cols", b"num") => Some(Self::Count),
            _ => None,
        }
    }

    fn count(value: &str) -> Result<u32> {
        Ok(value.parse()?)
    }
    pub(super) fn decode(self, element: &[u8], attribute: &[u8], value: &str) -> Result<()> {
        match self {
            Self::Measurement => parse_twips_measure::<i32>(element, attribute, value).map(|_| ()),
            Self::Count => Self::count(value).map(|_| ()),
        }
    }
}

/// Column definition for multi-column layouts.
#[derive(Debug, Clone, PartialEq)]
pub struct CT_Column {
    /// Column width in twips
    pub width: Option<Twips>,
    /// Space after this column in twips
    pub space: Option<Twips>,
}

/// `CT_Columns` — Column layout configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct CT_Columns {
    /// Number of columns (if equal width)
    pub num: Option<u32>,
    /// Space between columns in twips (when equal width)
    pub space: Option<Twips>,
    /// Whether columns are equal width
    pub equal_width: Option<bool>,
    /// Separator line between columns
    pub sep: Option<bool>,
    /// Individual column definitions (for unequal widths)
    pub columns: Vec<CT_Column>,
}

/// `CT_PageNumberType` -- the M23 page-number restart with retained M24 state.
#[derive(Debug, Clone, PartialEq)]
pub struct CT_PageNumberType {
    /// Displayed page number at which this section starts.
    pub start: Option<u32>,
    /// Original `w:pgNumType` subtree, including unsupported attributes.
    #[doc(hidden)]
    pub raw_xml: Option<Vec<u8>>,
    /// Parsed start value used to detect an authored change.
    #[doc(hidden)]
    pub parsed_start: Option<u32>,
    /// Qualified name of the retained start attribute.
    #[doc(hidden)]
    pub start_attribute_name: Option<Vec<u8>>,
    /// In-scope Word prefix preferred when adding a missing start attribute.
    #[doc(hidden)]
    pub start_insertion_prefix: Option<Vec<u8>>,
    /// Every inherited or local prefix reserved by the retained source scope.
    #[doc(hidden)]
    pub reserved_insertion_prefixes: Vec<Vec<u8>>,
}

impl CT_PageNumberType {
    /// Create an authored page-number restart.
    pub fn new(start: u32) -> Self {
        Self {
            start: Some(start),
            raw_xml: None,
            parsed_start: None,
            start_attribute_name: None,
            start_insertion_prefix: None,
            reserved_insertion_prefixes: Vec::new(),
        }
    }
}

impl Default for CT_Columns {
    fn default() -> Self {
        CT_Columns {
            num: Some(1),
            space: Some(Twips(720)),
            equal_width: Some(true),
            sep: None,
            columns: Vec::new(),
        }
    }
}

/// `ST_PageBorderZOrder` -- whether a page border draws in front of content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ST_PageBorderZOrder {
    Front,
    Back,
}

impl ST_PageBorderZOrder {
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "front" => Some(Self::Front),
            "back" => Some(Self::Back),
            _ => None,
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            Self::Front => "front",
            Self::Back => "back",
        }
    }
}

/// `ST_PageBorderDisplay` -- which pages of a section carry the page border.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ST_PageBorderDisplay {
    AllPages,
    FirstPage,
    NotFirstPage,
}

impl ST_PageBorderDisplay {
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "allPages" => Some(Self::AllPages),
            "firstPage" => Some(Self::FirstPage),
            "notFirstPage" => Some(Self::NotFirstPage),
            _ => None,
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            Self::AllPages => "allPages",
            Self::FirstPage => "firstPage",
            Self::NotFirstPage => "notFirstPage",
        }
    }
}

/// `ST_PageBorderOffset` -- what the page-border edge offsets are measured from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ST_PageBorderOffset {
    Page,
    Text,
}

impl ST_PageBorderOffset {
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "page" => Some(Self::Page),
            "text" => Some(Self::Text),
            _ => None,
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Text => "text",
        }
    }
}

/// `CT_PageBorders` -- the `w:pgBorders` frame drawn around a section's pages.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CT_PageBorders {
    /// `w:zOrder`, whether the frame draws in front of or behind page content.
    pub z_order: Option<ST_PageBorderZOrder>,
    /// `w:display`, which physical pages of the section receive the frame.
    pub display: Option<ST_PageBorderDisplay>,
    /// `w:offsetFrom`, the rectangle each edge offset is measured from.
    pub offset_from: Option<ST_PageBorderOffset>,
    pub top: Option<CT_BorderEdge>,
    pub left: Option<CT_BorderEdge>,
    pub bottom: Option<CT_BorderEdge>,
    pub right: Option<CT_BorderEdge>,
    /// Attributes this type does not model, in source order, written first.
    ///
    /// Values are serialized verbatim, so a caller storing one owns the
    /// escaping, exactly as `CT_BorderEdge::extra_attributes` does.
    pub extra_attributes: Vec<(String, String)>,
    /// Child elements this type does not model, in source order.
    pub extra_xml: Vec<Vec<u8>>,
}

/// `ST_LineNumberRestart` -- where a section's line numbering starts over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ST_LineNumberRestart {
    NewPage,
    NewSection,
    Continuous,
}

impl ST_LineNumberRestart {
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "newPage" => Some(Self::NewPage),
            "newSection" => Some(Self::NewSection),
            "continuous" => Some(Self::Continuous),
            _ => None,
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            Self::NewPage => "newPage",
            Self::NewSection => "newSection",
            Self::Continuous => "continuous",
        }
    }
}

/// `CT_LineNumber` -- the `w:lnNumType` margin line numbering of a section.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CT_LineNumber {
    /// `w:countBy`, the interval at which a number is printed.
    pub count_by: Option<u32>,
    /// `w:start`, the first line number of the section.
    pub start: Option<u32>,
    /// `w:distance`, the gap between the number and the text it labels.
    pub distance: Option<Twips>,
    /// `w:restart`, where numbering starts over.
    pub restart: Option<ST_LineNumberRestart>,
    /// Attributes this type does not model, in source order, written first.
    pub extra_attributes: Vec<(String, String)>,
}

/// `CT_PaperSource` -- the `w:paperSrc` printer trays a section prints from.
///
/// Tray selection happens in the printer driver, so this has no on-page
/// consequence and pagination is identical with and without it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CT_PaperSource {
    /// `w:first`, the tray feeding the section's first page.
    pub first: Option<u32>,
    /// `w:other`, the tray feeding every later page.
    pub other: Option<u32>,
    /// Attributes this type does not model, in source order, written first.
    pub extra_attributes: Vec<(String, String)>,
}

/// `ST_DocGrid` -- how a section's character grid constrains its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum ST_DocGrid {
    /// No grid. Lines and characters keep their ordinary metrics.
    Default,
    /// Line advance snaps to the grid pitch. Characters keep their advance.
    Lines,
    /// Line advance snaps to the pitch and characters snap to the char space.
    LinesAndChars,
    /// Characters snap to the char space, and line advance snaps too.
    SnapToChars,
}

impl ST_DocGrid {
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "default" => Some(Self::Default),
            "lines" => Some(Self::Lines),
            "linesAndChars" => Some(Self::LinesAndChars),
            "snapToChars" => Some(Self::SnapToChars),
            _ => None,
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Lines => "lines",
            Self::LinesAndChars => "linesAndChars",
            Self::SnapToChars => "snapToChars",
        }
    }

    /// Whether this grid type puts line advance on the grid pitch.
    ///
    /// `Default` is deliberately false. Keeping the default type off the grid
    /// path is what leaves every ungridded document on the arithmetic it
    /// already had, with no new floating-point step on the existing branch.
    pub fn snaps_lines(self) -> bool {
        matches!(self, Self::Lines | Self::LinesAndChars | Self::SnapToChars)
    }

    /// Whether this grid type puts per-character advance on the grid.
    pub fn snaps_characters(self) -> bool {
        matches!(self, Self::LinesAndChars | Self::SnapToChars)
    }
}

/// `CT_DocGrid` -- the `w:docGrid` character grid of a section.
///
/// `line_pitch` is the twip distance between two grid lines. `char_space` is
/// the twip addition to one East Asian character cell.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub struct CT_DocGrid {
    /// `w:type`, which of the grid constraints apply.
    pub grid_type: Option<ST_DocGrid>,
    /// `w:linePitch`, the grid line spacing in twips.
    pub line_pitch: Option<Twips>,
    /// `w:charSpace`, the grid character advance addition in twips.
    pub char_space: Option<i32>,
    /// Attributes this type does not model, in source order, written first.
    pub extra_attributes: Vec<(String, String)>,
}

/// Section-level note configuration, shared by `w:footnotePr` and `w:endnotePr`.
///
/// The values and the authoring surface are modeled. Their effect on marker
/// text, placement and restart is not applied yet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CT_NoteProperties {
    /// `w:pos/@w:val`, where the notes are placed.
    pub pos: Option<String>,
    /// `w:numFmt/@w:val`, the marker number format.
    pub num_fmt: Option<String>,
    /// `w:numStart/@w:val`, the first marker number.
    pub num_start: Option<u32>,
    /// `w:numRestart/@w:val`, where marker numbering starts over.
    pub num_restart: Option<String>,
    /// IDs of special note records selected by document settings.
    pub special_references: Vec<i32>,
    /// Unmodelled children retained at their schema insertion slots.
    ///
    /// A modelled child carrying an attribute this type does not read is kept
    /// here whole rather than typed, which is the choice `w:pgSz` already makes.
    #[doc(hidden)]
    pub extra_xml: Vec<(usize, Vec<u8>)>,
}

/// Value and occurrence anchor for one repeated section reference.
///
/// Distinguishable values follow their source predecessor or successor. Equal
/// duplicates are indistinguishable through the public `Vec` mutation surface,
/// so they resolve deterministically by their ordinal among equal current
/// values. The following anchor takes precedence when both neighbors resolve.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct CT_SectPrReferenceAnchor {
    reference: HdrFtrRef,
    source_occurrence: usize,
}

impl CT_SectPrReferenceAnchor {
    fn at(references: &[HdrFtrRef], index: usize) -> Option<Self> {
        let reference = references.get(index)?.clone();
        let source_occurrence = references[..index]
            .iter()
            .filter(|candidate| **candidate == reference)
            .count();
        Some(Self {
            reference,
            source_occurrence,
        })
    }

    fn resolve(&self, references: &[HdrFtrRef]) -> Option<usize> {
        references
            .iter()
            .enumerate()
            .filter(|(_, reference)| **reference == self.reference)
            .nth(self.source_occurrence)
            .map(|(index, _)| index)
    }
}

/// Retained section-child position, including repeated-child occurrence anchors.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub enum CT_SectPrRawPosition {
    /// A schema slot and modeled occurrence boundary.
    Schema { slot: usize, occurrence: usize },
    /// A boundary tied to neighboring header-reference values and occurrences.
    Header {
        preceding: Option<CT_SectPrReferenceAnchor>,
        following: Option<CT_SectPrReferenceAnchor>,
    },
    /// A boundary tied to neighboring footer-reference values and occurrences.
    Footer {
        preceding: Option<CT_SectPrReferenceAnchor>,
        following: Option<CT_SectPrReferenceAnchor>,
    },
}

/// `CT_SectPr` — Section properties (page size, margins, columns, orientation).
#[derive(Debug, Clone, PartialEq)]
#[allow(non_snake_case)]
pub struct CT_SectPr {
    /// Page width in twips
    pub page_width: Option<Twips>,
    /// Page height in twips
    pub page_height: Option<Twips>,
    /// Page orientation
    pub orientation: Option<ST_PageOrientation>,
    /// Top margin in twips
    pub margin_top: Option<Twips>,
    /// Right margin in twips
    pub margin_right: Option<Twips>,
    /// Bottom margin in twips
    pub margin_bottom: Option<Twips>,
    /// Left margin in twips
    pub margin_left: Option<Twips>,
    /// Gutter margin in twips
    pub gutter: Option<Twips>,
    /// Header distance from top edge in twips
    pub header_distance: Option<Twips>,
    /// Footer distance from bottom edge in twips
    pub footer_distance: Option<Twips>,
    /// Section break type
    pub section_type: Option<ST_SectionType>,
    /// Column layout
    pub columns: Option<CT_Columns>,
    /// Page-number restart. Unsupported format and chapter attributes remain raw.
    pub page_number: Option<CT_PageNumberType>,
    /// Section footnote configuration. Not applied to note rendering yet.
    ///
    /// Boxed, with the other large members, so a section stays cheap on the
    /// stack. `CT_SectPr` sits inside `CT_PPr`, which test threads build by
    /// value against a 2 MiB ceiling.
    pub footnote_pr: Option<Box<CT_NoteProperties>>,
    /// Section endnote configuration. Not applied to note rendering yet.
    pub endnote_pr: Option<Box<CT_NoteProperties>>,
    /// Printer tray selection. Round-tripped, with no effect on page geometry.
    pub paper_source: Option<Box<CT_PaperSource>>,
    /// Page border frame.
    pub page_borders: Option<Box<CT_PageBorders>>,
    /// Margin line numbering.
    pub line_numbers: Option<Box<CT_LineNumber>>,
    /// Vertical alignment of the body band within the page.
    pub vertical_alignment: Option<ST_VerticalJc>,
    /// `w:textDirection`. Authored and preserved here, projected by layout.
    pub text_direction: Option<String>,
    /// `w:docGrid`, the section character grid.
    ///
    /// Boxed with the other composite members so a section stays cheap on the
    /// stack. `CT_SectPr` sits inside `CT_PPr`, which test threads build by
    /// value against a 2 MiB ceiling.
    pub doc_grid: Option<Box<CT_DocGrid>>,
    /// Title page (different first page header/footer)
    pub title_pg: Option<bool>,
    /// Header references
    pub header_refs: Vec<HdrFtrRef>,
    /// Footer references
    pub footer_refs: Vec<HdrFtrRef>,
    /// Unknown child elements captured as raw XML.
    pub extra_xml: Vec<Vec<u8>>,
    /// Schema slots and modeled-child boundaries for retained child elements.
    #[doc(hidden)]
    pub extra_xml_positions: Vec<CT_SectPrRawPosition>,
    /// Prior section properties from the schema-final `w:sectPrChange`.
    pub change: Option<CT_Revision>,
}

#[allow(non_snake_case)]
impl CT_SectPr {
    fn empty() -> Self {
        Self {
            page_width: None,
            page_height: None,
            orientation: None,
            margin_top: None,
            margin_right: None,
            margin_bottom: None,
            margin_left: None,
            gutter: None,
            header_distance: None,
            footer_distance: None,
            section_type: None,
            columns: None,
            page_number: None,
            footnote_pr: None,
            endnote_pr: None,
            paper_source: None,
            page_borders: None,
            line_numbers: None,
            vertical_alignment: None,
            text_direction: None,
            doc_grid: None,
            title_pg: None,
            header_refs: Vec::new(),
            footer_refs: Vec::new(),
            extra_xml: Vec::new(),
            extra_xml_positions: Vec::new(),
            change: None,
        }
    }

    /// Report whether one raw section carrier retains root attributes.
    #[doc(hidden)]
    pub fn raw_position_is_root_attributes(position: &CT_SectPrRawPosition, raw: &[u8]) -> bool {
        matches!(
            position,
            CT_SectPrRawPosition::Schema {
                slot: usize::MAX,
                occurrence: usize::MAX
            }
        ) && is_root_attribute_record(raw)
    }

    pub(super) fn from_empty_root(root: &BytesStart<'_>, word_prefixes: &[String]) -> Result<Self> {
        let mut section = Self::empty();
        if let Some(record) = capture_root_attribute_record(root, word_prefixes)? {
            section.extra_xml.push(record);
            section
                .extra_xml_positions
                .push(CT_SectPrRawPosition::Schema {
                    slot: usize::MAX,
                    occurrence: usize::MAX,
                });
        }
        Ok(section)
    }

    /// Default US Letter page with 1-inch margins.
    pub fn default_letter() -> Self {
        CT_SectPr {
            page_width: Some(Twips(12240)),  // 8.5"
            page_height: Some(Twips(15840)), // 11"
            orientation: Some(ST_PageOrientation::Portrait),
            margin_top: Some(Twips(1440)),    // 1"
            margin_right: Some(Twips(1440)),  // 1"
            margin_bottom: Some(Twips(1440)), // 1"
            margin_left: Some(Twips(1440)),   // 1"
            gutter: Some(Twips(0)),
            header_distance: Some(Twips(720)),
            footer_distance: Some(Twips(720)),
            section_type: None,
            columns: None,
            page_number: None,
            footnote_pr: None,
            endnote_pr: None,
            paper_source: None,
            page_borders: None,
            line_numbers: None,
            vertical_alignment: None,
            text_direction: None,
            doc_grid: None,
            title_pg: None,
            header_refs: Vec::new(),
            footer_refs: Vec::new(),
            extra_xml: Vec::new(),
            extra_xml_positions: Vec::new(),
            change: None,
        }
    }

    /// Default A4 page with 1-inch margins.
    pub fn default_a4() -> Self {
        CT_SectPr {
            page_width: Some(Twips(11906)),  // 210mm
            page_height: Some(Twips(16838)), // 297mm
            orientation: Some(ST_PageOrientation::Portrait),
            margin_top: Some(Twips(1440)),
            margin_right: Some(Twips(1440)),
            margin_bottom: Some(Twips(1440)),
            margin_left: Some(Twips(1440)),
            gutter: Some(Twips(0)),
            header_distance: Some(Twips(720)),
            footer_distance: Some(Twips(720)),
            section_type: None,
            columns: None,
            page_number: None,
            footnote_pr: None,
            endnote_pr: None,
            paper_source: None,
            page_borders: None,
            line_numbers: None,
            vertical_alignment: None,
            text_direction: None,
            doc_grid: None,
            title_pg: None,
            header_refs: Vec::new(),
            footer_refs: Vec::new(),
            extra_xml: Vec::new(),
            extra_xml_positions: Vec::new(),
            change: None,
        }
    }

    pub fn from_xml(reader: &mut Reader<&[u8]>) -> Result<Self> {
        Self::from_xml_with_prefixes(reader, &["w".to_owned()])
    }

    pub(crate) fn from_xml_with_prefixes(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
    ) -> Result<Self> {
        Self::from_xml_with_prefixes_and_owner_bindings(reader, word_prefixes, &[])
    }

    pub(crate) fn from_xml_with_prefixes_and_owner_bindings(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
        owner_bindings: &[(String, String)],
    ) -> Result<Self> {
        Self::from_xml_with_prefixes_owner_bindings_and_root(
            reader,
            word_prefixes,
            owner_bindings,
            None,
        )
    }

    /// Admit one checked section owner before typed construction.
    #[doc(hidden)]
    pub fn from_xml_fragment_checked(xml: &[u8]) -> Result<Self> {
        admit_word_xml(xml, &[], None)?;
        crate::text::validate_word_owner_fragment(xml, b"sectPr", true)?;
        let mut reader = Reader::from_reader(xml);
        let mut buffer = Vec::new();
        loop {
            match reader.read_event_into(&mut buffer)? {
                Event::Start(root) => {
                    let prefixes = word_prefixes_at(&root, &[])?;
                    let bindings = local_namespace_overrides(&root, &prefixes)?;
                    return Self::from_xml_with_prefixes_owner_bindings_and_root(
                        &mut reader,
                        &prefixes,
                        &bindings,
                        Some(&root),
                    );
                }
                Event::Empty(root) => {
                    let prefixes = word_prefixes_at(&root, &[])?;
                    return Self::from_empty_root(&root, &prefixes);
                }
                _ => {}
            }
            buffer.clear();
        }
    }

    pub(crate) fn from_xml_with_prefixes_owner_bindings_and_root(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
        owner_bindings: &[(String, String)],
        root: Option<&BytesStart<'_>>,
    ) -> Result<Self> {
        let raw_owner_bindings =
            crate::text::bindings_outside_retained_root(owner_bindings, root, word_prefixes)?;
        // A page-number raw owner is also read independently by layout. Preserve
        // its actual inherited URI authority before section/part scope is lost.
        // The established fixed writer supplies verified canonical w already.
        let page_number_bindings =
            merged_owner_bindings(&namespace_bindings(word_prefixes), owner_bindings)
                .into_iter()
                .filter(|(prefix, uri)| prefix != "w" || uri != W_NS)
                .collect::<Vec<_>>();
        let mut sect = Self::empty();
        let mut change_raw_index = 0usize;
        let mut raw_position = (0usize, 0usize);
        let mut header_count = 0usize;
        let mut footer_count = 0usize;
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Empty(ref e)) => {
                    let name = e.name();
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    if is_word_element(name.as_ref(), b"pgSz", &prefixes) {
                        Self::parse_page_size_attributes(&mut sect, e, &prefixes)?;
                        raw_position = (4, 0);
                    } else if is_word_element(name.as_ref(), b"pgMar", &prefixes) {
                        Self::parse_page_margin_attributes(&mut sect, e, &prefixes)?;
                        raw_position = (5, 0);
                    } else if is_word_element(name.as_ref(), b"type", &prefixes) {
                        if let Some(val) = get_word_val_attr(e, &prefixes)? {
                            sect.section_type = ST_SectionType::from_str(&val).ok();
                        }
                        raw_position = (3, 0);
                    } else if is_word_element(name.as_ref(), b"pgNumType", &prefixes) {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_empty(e, reader.empty_writing())?,
                            &page_number_bindings,
                        )?;
                        if sect.page_number.is_none() {
                            sect.page_number =
                                Some(Self::parse_page_number(e, &prefixes, owner_bindings, raw)?);
                        } else {
                            sect.push_extra_xml(raw, 6, 1);
                        }
                        raw_position = (6, 1);
                    } else if is_word_element(name.as_ref(), b"cols", &prefixes) {
                        sect.columns = Some(Self::parse_cols_empty(e, &prefixes)?);
                        raw_position = (7, 0);
                    } else if is_word_element(name.as_ref(), b"footnotePr", &prefixes) {
                        if sect.footnote_pr.is_none() && e.attributes().next().is_none() {
                            sect.footnote_pr = Some(Box::default());
                        } else {
                            let raw = crate::text::raw_with_external_bindings(
                                &capture_empty(e, reader.empty_writing())?,
                                &raw_owner_bindings,
                            )?;
                            sect.push_extra_xml(raw, 2, 0);
                        }
                        raw_position = (2, 0);
                    } else if is_word_element(name.as_ref(), b"endnotePr", &prefixes) {
                        if sect.endnote_pr.is_none() && e.attributes().next().is_none() {
                            sect.endnote_pr = Some(Box::default());
                        } else {
                            let raw = crate::text::raw_with_external_bindings(
                                &capture_empty(e, reader.empty_writing())?,
                                &raw_owner_bindings,
                            )?;
                            sect.push_extra_xml(raw, 2, 1);
                        }
                        raw_position = (2, 1);
                    } else if is_word_element(name.as_ref(), b"paperSrc", &prefixes) {
                        if sect.paper_source.is_none() {
                            sect.paper_source =
                                Some(Box::new(Self::parse_paper_source(e, &prefixes)?));
                        } else {
                            let raw = crate::text::raw_with_external_bindings(
                                &capture_empty(e, reader.empty_writing())?,
                                &raw_owner_bindings,
                            )?;
                            sect.push_extra_xml(raw, 5, 1);
                        }
                        raw_position = (5, 1);
                    } else if is_word_element(name.as_ref(), b"pgBorders", &prefixes) {
                        if sect.page_borders.is_none() {
                            sect.page_borders =
                                Some(Box::new(Self::parse_page_border_attrs(e, &prefixes)?));
                        } else {
                            let raw = crate::text::raw_with_external_bindings(
                                &capture_empty(e, reader.empty_writing())?,
                                &raw_owner_bindings,
                            )?;
                            sect.push_extra_xml(raw, 5, 2);
                        }
                        raw_position = (5, 2);
                    } else if is_word_element(name.as_ref(), b"lnNumType", &prefixes) {
                        if sect.line_numbers.is_none() {
                            sect.line_numbers =
                                Some(Box::new(Self::parse_line_number(e, &prefixes)?));
                        } else {
                            let raw = crate::text::raw_with_external_bindings(
                                &capture_empty(e, reader.empty_writing())?,
                                &raw_owner_bindings,
                            )?;
                            sect.push_extra_xml(raw, 5, 3);
                        }
                        raw_position = (5, 3);
                    } else if is_word_element(name.as_ref(), b"vAlign", &prefixes) {
                        let typed = sole_val_attribute(e, &prefixes)?
                            .filter(|value| ST_VerticalJc::from_str(value).to_str() == value)
                            .map(|value| ST_VerticalJc::from_str(&value));
                        match typed {
                            Some(value) if sect.vertical_alignment.is_none() => {
                                sect.vertical_alignment = Some(value);
                            }
                            _ => {
                                let raw = crate::text::raw_with_external_bindings(
                                    &capture_empty(e, reader.empty_writing())?,
                                    &raw_owner_bindings,
                                )?;
                                sect.push_extra_xml(raw, 7, 0);
                            }
                        }
                        raw_position = (7, 1);
                    } else if is_word_element(name.as_ref(), b"textDirection", &prefixes) {
                        match sole_val_attribute(e, &prefixes)? {
                            Some(value) if sect.text_direction.is_none() => {
                                sect.text_direction = Some(value);
                            }
                            _ => {
                                let raw = crate::text::raw_with_external_bindings(
                                    &capture_empty(e, reader.empty_writing())?,
                                    &raw_owner_bindings,
                                )?;
                                sect.push_extra_xml(raw, 8, 0);
                            }
                        }
                        raw_position = (8, 1);
                    } else if is_word_element(name.as_ref(), b"docGrid", &prefixes) {
                        if sect.doc_grid.is_none() {
                            sect.doc_grid = Some(Box::new(Self::parse_doc_grid(e, &prefixes)?));
                        } else {
                            // The schema allows one. A second is retained
                            // after the typed one rather than before it, the
                            // way a second `w:paperSrc` already is.
                            let raw = crate::text::raw_with_external_bindings(
                                &capture_empty(e, reader.empty_writing())?,
                                &raw_owner_bindings,
                            )?;
                            sect.push_extra_xml(raw, 8, 2);
                        }
                        raw_position = (8, 2);
                    } else if is_word_element(name.as_ref(), b"headerReference", &prefixes) {
                        let mut hdr_type = HdrFtrType::Default;
                        let mut rel_id = String::new();
                        for attr in e.attributes() {
                            let attr = attr?;
                            let key = attr.key.as_ref();
                            let val = oxoffice_core::xml::attribute_text(&attr)?;
                            if is_word_attribute(key, b"type", &prefixes) {
                                hdr_type = HdrFtrType::from_str(&val);
                            } else if attribute_in_namespace(
                                key,
                                b"id",
                                crate::namespace::R_NS,
                                &prefixes,
                            ) {
                                rel_id = val.to_string();
                            }
                        }
                        if !rel_id.is_empty() {
                            sect.push_story_reference(HdrFtrRef::new(hdr_type, rel_id), true);
                            header_count += 1;
                        }
                        raw_position = (0, header_count);
                    } else if is_word_element(name.as_ref(), b"footerReference", &prefixes) {
                        let mut ftr_type = HdrFtrType::Default;
                        let mut rel_id = String::new();
                        for attr in e.attributes() {
                            let attr = attr?;
                            let key = attr.key.as_ref();
                            let val = oxoffice_core::xml::attribute_text(&attr)?;
                            if is_word_attribute(key, b"type", &prefixes) {
                                ftr_type = HdrFtrType::from_str(&val);
                            } else if attribute_in_namespace(
                                key,
                                b"id",
                                crate::namespace::R_NS,
                                &prefixes,
                            ) {
                                rel_id = val.to_string();
                            }
                        }
                        if !rel_id.is_empty() {
                            sect.push_story_reference(HdrFtrRef::new(ftr_type, rel_id), false);
                            footer_count += 1;
                        }
                        raw_position = (1, footer_count);
                    } else if is_word_element(name.as_ref(), b"titlePg", &prefixes) {
                        sect.title_pg = Some(parse_word_toggle(e, &prefixes)?);
                        raw_position = (8, 0);
                    } else if is_word_element(name.as_ref(), b"sectPrChange", &prefixes) {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_empty(e, reader.empty_writing())?,
                            &raw_owner_bindings,
                        )?;
                        if let Some(revision) = CT_Revision::from_raw(raw.clone(), &prefixes) {
                            if let Some(previous) = sect.change.replace(revision) {
                                sect.extra_xml
                                    .insert(change_raw_index, previous.into_raw_xml());
                                sect.extra_xml_positions.insert(
                                    change_raw_index,
                                    CT_SectPrRawPosition::Schema {
                                        slot: 9,
                                        occurrence: 0,
                                    },
                                );
                            }
                            change_raw_index = sect.extra_xml.len();
                        } else {
                            sect.push_extra_xml(raw, 9, 0);
                        }
                        raw_position = (9, 0);
                    } else {
                        // Capture unknown empty elements
                        let position = Self::raw_child_schema_slot(name.as_ref(), &prefixes)
                            .unwrap_or(raw_position);
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_empty(e, reader.empty_writing())?,
                            &raw_owner_bindings,
                        )?;
                        sect.push_extra_xml(raw, position.0, position.1);
                        raw_position = position;
                    }
                }
                Ok(Event::Start(ref e)) => {
                    let name = e.name();
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    if is_word_element(name.as_ref(), b"pgSz", &prefixes) {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_element(reader, e)?,
                            &raw_owner_bindings,
                        )?;
                        if raw_element_has_child_content(&raw)?
                            || !page_size_attributes_are_modeled(e, &prefixes)?
                        {
                            sect.push_extra_xml(raw, 4, 0);
                        } else {
                            Self::parse_page_size_attributes(&mut sect, e, &prefixes)?;
                        }
                        raw_position = (4, 0);
                    } else if is_word_element(name.as_ref(), b"pgMar", &prefixes) {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_element(reader, e)?,
                            &raw_owner_bindings,
                        )?;
                        if raw_element_has_child_content(&raw)?
                            || !page_margin_attributes_are_modeled(e, &prefixes)?
                        {
                            sect.push_extra_xml(raw, 5, 0);
                        } else {
                            Self::parse_page_margin_attributes(&mut sect, e, &prefixes)?;
                        }
                        raw_position = (5, 0);
                    } else if is_word_element(name.as_ref(), b"headerReference", &prefixes) {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_element(reader, e)?,
                            &raw_owner_bindings,
                        )?;
                        if raw_element_has_child_content(&raw)?
                            || !story_reference_attributes_are_modeled(e, &prefixes)?
                        {
                            sect.push_extra_xml(raw, 0, header_count);
                        } else if let Some(reference) = Self::parse_story_reference(e, &prefixes)? {
                            sect.push_story_reference(reference, true);
                            header_count += 1;
                        }
                        raw_position = (0, header_count);
                    } else if is_word_element(name.as_ref(), b"footerReference", &prefixes) {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_element(reader, e)?,
                            &raw_owner_bindings,
                        )?;
                        if raw_element_has_child_content(&raw)?
                            || !story_reference_attributes_are_modeled(e, &prefixes)?
                        {
                            sect.push_extra_xml(raw, 1, footer_count);
                        } else if let Some(reference) = Self::parse_story_reference(e, &prefixes)? {
                            sect.push_story_reference(reference, false);
                            footer_count += 1;
                        }
                        raw_position = (1, footer_count);
                    } else if is_word_element(name.as_ref(), b"cols", &prefixes) {
                        sect.columns = Some(Self::parse_cols_start(reader, e, &prefixes)?);
                        raw_position = (7, 0);
                    } else if is_word_element(name.as_ref(), b"footnotePr", &prefixes) {
                        if sect.footnote_pr.is_none() && e.attributes().next().is_none() {
                            sect.footnote_pr = Some(Box::new(Self::parse_note_properties(
                                reader,
                                &prefixes,
                                owner_bindings,
                                b"footnotePr",
                            )?));
                        } else {
                            let raw = crate::text::raw_with_external_bindings(
                                &capture_element(reader, e)?,
                                &raw_owner_bindings,
                            )?;
                            sect.push_extra_xml(raw, 2, 0);
                        }
                        raw_position = (2, 0);
                    } else if is_word_element(name.as_ref(), b"endnotePr", &prefixes) {
                        if sect.endnote_pr.is_none() && e.attributes().next().is_none() {
                            sect.endnote_pr = Some(Box::new(Self::parse_note_properties(
                                reader,
                                &prefixes,
                                owner_bindings,
                                b"endnotePr",
                            )?));
                        } else {
                            let raw = crate::text::raw_with_external_bindings(
                                &capture_element(reader, e)?,
                                &raw_owner_bindings,
                            )?;
                            sect.push_extra_xml(raw, 2, 1);
                        }
                        raw_position = (2, 1);
                    } else if is_word_element(name.as_ref(), b"pgBorders", &prefixes) {
                        if sect.page_borders.is_none() {
                            sect.page_borders = Some(Box::new(Self::parse_page_borders(
                                reader,
                                e,
                                &prefixes,
                                owner_bindings,
                            )?));
                        } else {
                            let raw = crate::text::raw_with_external_bindings(
                                &capture_element(reader, e)?,
                                &raw_owner_bindings,
                            )?;
                            sect.push_extra_xml(raw, 5, 2);
                        }
                        raw_position = (5, 2);
                    } else if is_word_element(name.as_ref(), b"paperSrc", &prefixes) {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_element(reader, e)?,
                            &raw_owner_bindings,
                        )?;
                        sect.push_extra_xml(raw, 5, 0);
                        raw_position = (5, 1);
                    } else if is_word_element(name.as_ref(), b"lnNumType", &prefixes) {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_element(reader, e)?,
                            &raw_owner_bindings,
                        )?;
                        sect.push_extra_xml(raw, 5, 2);
                        raw_position = (5, 3);
                    } else if is_word_element(name.as_ref(), b"vAlign", &prefixes) {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_element(reader, e)?,
                            &raw_owner_bindings,
                        )?;
                        sect.push_extra_xml(raw, 7, 0);
                        raw_position = (7, 1);
                    } else if is_word_element(name.as_ref(), b"textDirection", &prefixes) {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_element(reader, e)?,
                            &raw_owner_bindings,
                        )?;
                        sect.push_extra_xml(raw, 8, 0);
                        raw_position = (8, 1);
                    } else if is_word_element(name.as_ref(), b"docGrid", &prefixes) {
                        // The schema makes `w:docGrid` empty. One that carries
                        // children is a producer extension, so it stays raw at
                        // the slot the typed element would have written.
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_element(reader, e)?,
                            &raw_owner_bindings,
                        )?;
                        sect.push_extra_xml(raw, 8, 1);
                        raw_position = (8, 2);
                    } else if is_word_element(name.as_ref(), b"pgNumType", &prefixes) {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_element(reader, e)?,
                            &page_number_bindings,
                        )?;
                        if sect.page_number.is_none() {
                            sect.page_number =
                                Some(Self::parse_page_number(e, &prefixes, owner_bindings, raw)?);
                        } else {
                            sect.push_extra_xml(raw, 6, 1);
                        }
                        raw_position = (6, 1);
                    } else if is_word_element(name.as_ref(), b"sectPrChange", &prefixes) {
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_element(reader, e)?,
                            &raw_owner_bindings,
                        )?;
                        if let Some(revision) = CT_Revision::from_raw(raw.clone(), &prefixes) {
                            if let Some(previous) = sect.change.replace(revision) {
                                sect.extra_xml
                                    .insert(change_raw_index, previous.into_raw_xml());
                                sect.extra_xml_positions.insert(
                                    change_raw_index,
                                    CT_SectPrRawPosition::Schema {
                                        slot: 9,
                                        occurrence: 0,
                                    },
                                );
                            }
                            change_raw_index = sect.extra_xml.len();
                        } else {
                            sect.push_extra_xml(raw, 9, 0);
                        }
                        raw_position = (9, 0);
                    } else {
                        // Capture unknown start elements as raw XML
                        let position = Self::raw_child_schema_slot(name.as_ref(), &prefixes)
                            .unwrap_or(raw_position);
                        let raw = crate::text::raw_with_external_bindings(
                            &capture_element(reader, e)?,
                            &raw_owner_bindings,
                        )?;
                        sect.push_extra_xml(raw, position.0, position.1);
                        raw_position = position;
                    }
                }
                Ok(Event::End(ref e)) if matches_local_name(e.name().as_ref(), b"sectPr") => {
                    break;
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(e.into()),
                _ => {}
            }
            buf.clear();
        }

        if let Some(record) = root
            .map(|root| capture_root_attribute_record(root, word_prefixes))
            .transpose()?
            .flatten()
        {
            sect.extra_xml.push(record);
            sect.extra_xml_positions.push(CT_SectPrRawPosition::Schema {
                slot: usize::MAX,
                occurrence: usize::MAX,
            });
        }
        sect.bind_story_reference_positions();
        Ok(sect)
    }

    fn parse_page_size_attributes(
        sect: &mut Self,
        element: &BytesStart<'_>,
        word_prefixes: &[String],
    ) -> Result<()> {
        for attribute in element.attributes() {
            let attribute = attribute?;
            let key = attribute.key.as_ref();
            let value = oxoffice_core::xml::attribute_text(&attribute)?;
            let value: &str = &value;
            let measurement = || parse_twips_measure(element.name().as_ref(), key, value);
            if is_word_attribute(key, b"w", word_prefixes) {
                sect.page_width = Some(Twips(measurement()?));
            } else if is_word_attribute(key, b"h", word_prefixes) {
                sect.page_height = Some(Twips(measurement()?));
            } else if is_word_attribute(key, b"orient", word_prefixes) {
                sect.orientation = ST_PageOrientation::from_str(value).ok();
            }
        }
        Ok(())
    }

    fn parse_page_margin_attributes(
        sect: &mut Self,
        element: &BytesStart<'_>,
        word_prefixes: &[String],
    ) -> Result<()> {
        for attribute in element.attributes() {
            let attribute = attribute?;
            let key = attribute.key.as_ref();
            let field = if is_word_attribute(key, b"top", word_prefixes) {
                &mut sect.margin_top
            } else if is_word_attribute(key, b"right", word_prefixes)
                || is_word_attribute(key, b"end", word_prefixes)
            {
                &mut sect.margin_right
            } else if is_word_attribute(key, b"bottom", word_prefixes) {
                &mut sect.margin_bottom
            } else if is_word_attribute(key, b"left", word_prefixes)
                || is_word_attribute(key, b"start", word_prefixes)
            {
                &mut sect.margin_left
            } else if is_word_attribute(key, b"gutter", word_prefixes) {
                &mut sect.gutter
            } else if is_word_attribute(key, b"header", word_prefixes) {
                &mut sect.header_distance
            } else if is_word_attribute(key, b"footer", word_prefixes) {
                &mut sect.footer_distance
            } else {
                continue;
            };
            let value = oxoffice_core::xml::attribute_text(&attribute)?;
            let value: &str = &value;
            *field = Some(Twips(parse_twips_measure(
                element.name().as_ref(),
                key,
                value,
            )?));
        }
        Ok(())
    }

    fn parse_story_reference(
        element: &BytesStart<'_>,
        word_prefixes: &[String],
    ) -> Result<Option<HdrFtrRef>> {
        let mut hdr_ftr_type = HdrFtrType::Default;
        let mut relationship_id = None;
        for attribute in element.attributes() {
            let attribute = attribute?;
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())?
                .into_owned();
            if is_word_attribute(attribute.key.as_ref(), b"type", word_prefixes) {
                hdr_ftr_type = HdrFtrType::from_str(&value);
            } else if attribute_in_namespace(
                attribute.key.as_ref(),
                b"id",
                crate::namespace::R_NS,
                word_prefixes,
            ) {
                relationship_id = Some(value);
            }
        }
        Ok(relationship_id.map(|rel_id| HdrFtrRef::new(hdr_ftr_type, rel_id)))
    }

    /// Append a header (or footer) reference read from the source, with its
    /// place among all the header and footer references of the section.
    fn push_story_reference(&mut self, mut reference: HdrFtrRef, header: bool) {
        reference.source_index = Some(self.header_refs.len() + self.footer_refs.len());
        if header {
            self.header_refs.push(reference);
        } else {
            self.footer_refs.push(reference);
        }
    }

    /// The order the header and footer references are written in, each as
    /// whether it is a header and its index in its list. Each list keeps its
    /// own order. References read together keep the order they were read
    /// in, Word's interleaving included. A new reference goes just before
    /// the next reference read in its list, and those at the end of their
    /// list after every reference read, header then footer for each type
    /// in the order default, first, even: the order Word writes a default
    /// and a first-page header and footer in.
    fn story_reference_order(&self) -> Vec<(bool, usize)> {
        // (place of the next reference read, read after new, type rank)
        fn keys(references: &[HdrFtrRef], header: bool) -> Vec<(usize, u8, u8)> {
            let mut next_read = usize::MAX;
            let mut keys = vec![(0, 0, 0); references.len()];
            for (index, reference) in references.iter().enumerate().rev() {
                keys[index] = match reference.source_index {
                    Some(source) => {
                        next_read = source;
                        (source, 1, 0)
                    }
                    None => {
                        let rank = match reference.hdr_ftr_type {
                            HdrFtrType::Default => 0,
                            HdrFtrType::First => 2,
                            HdrFtrType::Even => 4,
                        } + u8::from(!header);
                        (next_read, 0, rank)
                    }
                };
            }
            keys
        }
        let headers = keys(&self.header_refs, true);
        let footers = keys(&self.footer_refs, false);
        let (mut header, mut footer) = (0, 0);
        let mut order = Vec::with_capacity(headers.len() + footers.len());
        while header < headers.len() || footer < footers.len() {
            if footer == footers.len()
                || (header < headers.len() && headers[header] <= footers[footer])
            {
                order.push((true, header));
                header += 1;
            } else {
                order.push((false, footer));
                footer += 1;
            }
        }
        order
    }

    fn parse_page_number(
        e: &BytesStart<'_>,
        word_prefixes: &[String],
        owner_bindings: &[(String, String)],
        raw_xml: Vec<u8>,
    ) -> Result<CT_PageNumberType> {
        let mut start = None;
        let mut start_attribute_name = None;
        for attr in e.attributes() {
            let attr = attr?;
            if is_word_attribute(attr.key.as_ref(), b"start", word_prefixes) {
                start = attr
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, e.decoder())?
                    .parse()
                    .ok();
                start_attribute_name = Some(attr.key.as_ref().to_vec());
            }
        }
        let element_name = e.name();
        let element_prefix = element_name
            .as_ref()
            .iter()
            .position(|byte| *byte == b':')
            .map(|separator| &element_name.as_ref()[..separator]);
        let start_insertion_prefix = element_prefix
            .filter(|prefix| {
                !prefix.is_empty()
                    && word_prefixes
                        .iter()
                        .any(|candidate| candidate.as_bytes() == *prefix)
            })
            .or_else(|| {
                word_prefixes
                    .iter()
                    .find(|prefix| !prefix.is_empty() && !prefix.starts_with('\0'))
                    .map(String::as_bytes)
            })
            .map(<[u8]>::to_vec);
        let mut reserved_insertion_prefixes = owner_bindings
            .iter()
            .map(|(prefix, _)| prefix.as_bytes().to_vec())
            .collect::<Vec<_>>();
        reserved_insertion_prefixes.extend(
            namespace_bindings(word_prefixes)
                .into_iter()
                .map(|(prefix, _)| prefix.into_bytes()),
        );
        reserved_insertion_prefixes.sort();
        reserved_insertion_prefixes.dedup();
        Ok(CT_PageNumberType {
            start,
            raw_xml: Some(raw_xml),
            parsed_start: start,
            start_attribute_name,
            start_insertion_prefix,
            reserved_insertion_prefixes,
        })
    }

    fn push_extra_xml(&mut self, raw: Vec<u8>, slot: usize, occurrence: usize) {
        self.extra_xml.push(raw);
        self.extra_xml_positions
            .push(CT_SectPrRawPosition::Schema { slot, occurrence });
    }

    fn bind_story_reference_positions(&mut self) {
        for position in &mut self.extra_xml_positions {
            let (slot, occurrence) = match position {
                CT_SectPrRawPosition::Schema { slot, occurrence } => (*slot, *occurrence),
                _ => continue,
            };
            let (references, is_header) = match slot {
                0 => (&self.header_refs, true),
                1 => (&self.footer_refs, false),
                _ => continue,
            };
            let preceding = occurrence
                .checked_sub(1)
                .and_then(|index| CT_SectPrReferenceAnchor::at(references, index));
            let following = CT_SectPrReferenceAnchor::at(references, occurrence);
            *position = if is_header {
                CT_SectPrRawPosition::Header {
                    preceding,
                    following,
                }
            } else {
                CT_SectPrRawPosition::Footer {
                    preceding,
                    following,
                }
            };
        }
    }

    /// Schema slot and sub-slot for a `w:sectPr` child this type keeps raw.
    ///
    /// The sub-slot orders a retained child against the modeled siblings that
    /// share its slot, which is what keeps `xsd:sequence` intact now that
    /// `w:vAlign` and `w:textDirection` sit between them.
    fn raw_child_schema_slot(name: &[u8], word_prefixes: &[String]) -> Option<(usize, usize)> {
        let local = name.rsplit(|byte| *byte == b':').next().unwrap_or(name);
        if !is_word_element(name, local, word_prefixes) {
            return None;
        }
        match local {
            b"formProt" => Some((7, 0)),
            b"noEndnote" => Some((7, 1)),
            b"bidi" | b"rtlGutter" => Some((8, 1)),
            b"printerSettings" => Some((8, 2)),
            _ => None,
        }
    }

    /// Read `w:docGrid`, keeping every attribute this type does not model.
    fn parse_doc_grid(e: &BytesStart, word_prefixes: &[String]) -> Result<CT_DocGrid> {
        let mut grid = CT_DocGrid::default();
        for attribute in e.attributes() {
            let attribute = attribute?;
            let key = attribute.key.as_ref();
            let value = oxoffice_core::xml::attribute_text(&attribute)?;
            let value: &str = &value;
            let modeled = if is_word_attribute(key, b"type", word_prefixes) {
                match ST_DocGrid::from_str(value) {
                    Some(grid_type) => {
                        grid.grid_type = Some(grid_type);
                        true
                    }
                    None => false,
                }
            } else if is_word_attribute(key, b"linePitch", word_prefixes) {
                match value.parse() {
                    Ok(parsed) => {
                        grid.line_pitch = Some(Twips(parsed));
                        true
                    }
                    Err(_) => false,
                }
            } else if is_word_attribute(key, b"charSpace", word_prefixes) {
                match value.parse() {
                    Ok(parsed) => {
                        grid.char_space = Some(parsed);
                        true
                    }
                    Err(_) => false,
                }
            } else {
                false
            };
            if !modeled {
                grid.extra_attributes.push((
                    std::str::from_utf8(key)?.to_owned(),
                    std::str::from_utf8(&attribute.value)?.to_owned(),
                ));
            }
        }
        Ok(grid)
    }

    fn parse_paper_source(e: &BytesStart, word_prefixes: &[String]) -> Result<CT_PaperSource> {
        let mut source = CT_PaperSource::default();
        for attribute in e.attributes() {
            let attribute = attribute?;
            let key = attribute.key.as_ref();
            let value = oxoffice_core::xml::attribute_text(&attribute)?;
            let value: &str = &value;
            if is_word_attribute(key, b"first", word_prefixes) {
                store_unsigned_attribute(
                    value,
                    &attribute,
                    &mut source.first,
                    &mut source.extra_attributes,
                )?;
            } else if is_word_attribute(key, b"other", word_prefixes) {
                store_unsigned_attribute(
                    value,
                    &attribute,
                    &mut source.other,
                    &mut source.extra_attributes,
                )?;
            } else {
                source.extra_attributes.push((
                    std::str::from_utf8(key)?.to_owned(),
                    std::str::from_utf8(&attribute.value)?.to_owned(),
                ));
            }
        }
        Ok(source)
    }

    fn parse_line_number(e: &BytesStart, word_prefixes: &[String]) -> Result<CT_LineNumber> {
        let mut numbering = CT_LineNumber::default();
        for attribute in e.attributes() {
            let attribute = attribute?;
            let key = attribute.key.as_ref();
            let value = oxoffice_core::xml::attribute_text(&attribute)?;
            let value: &str = &value;
            if is_word_attribute(key, b"countBy", word_prefixes) {
                store_unsigned_attribute(
                    value,
                    &attribute,
                    &mut numbering.count_by,
                    &mut numbering.extra_attributes,
                )?;
            } else if is_word_attribute(key, b"start", word_prefixes) {
                store_unsigned_attribute(
                    value,
                    &attribute,
                    &mut numbering.start,
                    &mut numbering.extra_attributes,
                )?;
            } else if is_word_attribute(key, b"distance", word_prefixes) {
                match parse_twips_measure(e.name().as_ref(), key, value) {
                    Ok(parsed) => numbering.distance = Some(Twips(parsed)),
                    Err(_) => numbering.extra_attributes.push((
                        std::str::from_utf8(key)?.to_owned(),
                        std::str::from_utf8(&attribute.value)?.to_owned(),
                    )),
                }
            } else if is_word_attribute(key, b"restart", word_prefixes) {
                match ST_LineNumberRestart::from_str(value) {
                    Some(parsed) => numbering.restart = Some(parsed),
                    None => numbering.extra_attributes.push((
                        std::str::from_utf8(key)?.to_owned(),
                        std::str::from_utf8(&attribute.value)?.to_owned(),
                    )),
                }
            } else {
                numbering.extra_attributes.push((
                    std::str::from_utf8(key)?.to_owned(),
                    std::str::from_utf8(&attribute.value)?.to_owned(),
                ));
            }
        }
        Ok(numbering)
    }

    fn parse_page_border_attrs(e: &BytesStart, word_prefixes: &[String]) -> Result<CT_PageBorders> {
        let mut borders = CT_PageBorders::default();
        for attribute in e.attributes() {
            let attribute = attribute?;
            let key = attribute.key.as_ref();
            let value = oxoffice_core::xml::attribute_text(&attribute)?;
            let value: &str = &value;
            let retained = if is_word_attribute(key, b"zOrder", word_prefixes) {
                borders.z_order = ST_PageBorderZOrder::from_str(value);
                borders.z_order.is_none()
            } else if is_word_attribute(key, b"display", word_prefixes) {
                borders.display = ST_PageBorderDisplay::from_str(value);
                borders.display.is_none()
            } else if is_word_attribute(key, b"offsetFrom", word_prefixes) {
                borders.offset_from = ST_PageBorderOffset::from_str(value);
                borders.offset_from.is_none()
            } else {
                true
            };
            if retained {
                borders.extra_attributes.push((
                    std::str::from_utf8(key)?.to_owned(),
                    std::str::from_utf8(&attribute.value)?.to_owned(),
                ));
            }
        }
        Ok(borders)
    }

    fn parse_page_borders(
        reader: &mut Reader<&[u8]>,
        e: &BytesStart,
        word_prefixes: &[String],
        owner_bindings: &[(String, String)],
    ) -> Result<CT_PageBorders> {
        let mut borders = Self::parse_page_border_attrs(e, word_prefixes)?;
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Empty(ref child)) => {
                    let name = child.name();
                    let prefixes = word_prefixes_at(child, word_prefixes)?;
                    let edge = |child: &BytesStart| {
                        CT_BorderEdge::from_xml_attrs_with_prefixes(child, &prefixes)
                    };
                    let slot = if is_word_element(name.as_ref(), b"top", &prefixes) {
                        Some(&mut borders.top)
                    } else if is_word_element(name.as_ref(), b"left", &prefixes) {
                        Some(&mut borders.left)
                    } else if is_word_element(name.as_ref(), b"bottom", &prefixes) {
                        Some(&mut borders.bottom)
                    } else if is_word_element(name.as_ref(), b"right", &prefixes) {
                        Some(&mut borders.right)
                    } else {
                        None
                    };
                    if let Some(slot) = slot {
                        if slot.is_some() {
                            return Err(OxmlError::InvalidValue(
                                "duplicate recognized page border edge".into(),
                            ));
                        }
                        *slot = Some(edge(child)?);
                    } else {
                        borders
                            .extra_xml
                            .push(crate::text::raw_with_external_bindings(
                                &capture_empty(child, reader.empty_writing())?,
                                owner_bindings,
                            )?);
                    }
                }
                Ok(Event::Start(ref child)) => {
                    let prefixes = word_prefixes_at(child, word_prefixes)?;
                    let name = child.name();
                    let slot = if is_word_element(name.as_ref(), b"top", &prefixes) {
                        Some(&mut borders.top)
                    } else if is_word_element(name.as_ref(), b"left", &prefixes) {
                        Some(&mut borders.left)
                    } else if is_word_element(name.as_ref(), b"bottom", &prefixes) {
                        Some(&mut borders.bottom)
                    } else if is_word_element(name.as_ref(), b"right", &prefixes) {
                        Some(&mut borders.right)
                    } else {
                        None
                    };
                    let mut edge = if slot.is_some() {
                        Some(CT_BorderEdge::from_xml_attrs_with_prefixes(
                            child, &prefixes,
                        )?)
                    } else {
                        None
                    };
                    let mut bindings = owner_bindings.to_vec();
                    for (prefix, uri) in namespace_bindings(&prefixes) {
                        bindings.retain(|(name, _)| name != &prefix);
                        bindings.push((prefix, uri));
                    }
                    for prefix in prefixes.iter().filter(|prefix| !prefix.starts_with('\0')) {
                        if !bindings.iter().any(|(name, _)| name == prefix) {
                            bindings.push((
                                prefix.clone(),
                                "http://schemas.openxmlformats.org/wordprocessingml/2006/main"
                                    .into(),
                            ));
                        }
                    }
                    let raw = crate::text::raw_with_external_bindings(
                        &capture_element(reader, child)?,
                        if slot.is_some() {
                            &bindings
                        } else {
                            owner_bindings
                        },
                    )?;
                    if let Some(slot) = slot {
                        if slot.is_some() {
                            return Err(OxmlError::InvalidValue(
                                "duplicate recognized page border edge".into(),
                            ));
                        }
                        let mut edge = edge.take().unwrap();
                        edge.source_xml = Some(raw);
                        *slot = Some(edge);
                    } else {
                        borders.extra_xml.push(raw);
                    }
                }
                Ok(Event::End(ref end))
                    if matches_local_name(end.name().as_ref(), b"pgBorders") =>
                {
                    break;
                }
                Ok(Event::Eof) => break,
                Err(error) => return Err(error.into()),
                _ => {}
            }
            buf.clear();
        }
        Ok(borders)
    }

    pub(crate) fn parse_note_properties(
        reader: &mut Reader<&[u8]>,
        word_prefixes: &[String],
        owner_bindings: &[(String, String)],
        tag: &[u8],
    ) -> Result<CT_NoteProperties> {
        let mut properties = CT_NoteProperties::default();
        let mut slot = 0usize;
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Empty(ref child)) => {
                    let name = child.name();
                    let prefixes = word_prefixes_at(child, word_prefixes)?;
                    let modeled = if is_word_element(name.as_ref(), b"pos", &prefixes) {
                        slot = 1;
                        sole_val_attribute(child, &prefixes)?
                            .map(|value| properties.pos = Some(value))
                    } else if is_word_element(name.as_ref(), b"numFmt", &prefixes) {
                        slot = 2;
                        sole_val_attribute(child, &prefixes)?
                            .map(|value| properties.num_fmt = Some(value))
                    } else if is_word_element(name.as_ref(), b"numStart", &prefixes) {
                        slot = 3;
                        sole_val_attribute(child, &prefixes)?
                            .and_then(|value| value.parse().ok())
                            .map(|value| properties.num_start = Some(value))
                    } else if is_word_element(name.as_ref(), b"numRestart", &prefixes) {
                        slot = 4;
                        sole_val_attribute(child, &prefixes)?
                            .map(|value| properties.num_restart = Some(value))
                    } else if is_word_element(
                        name.as_ref(),
                        if matches_local_name(tag, b"footnotePr") {
                            b"footnote".as_slice()
                        } else {
                            b"endnote".as_slice()
                        },
                        &prefixes,
                    ) {
                        slot = 5;
                        child
                            .attributes()
                            .filter_map(|attribute| attribute.ok())
                            .find(|attribute| {
                                is_word_attribute(attribute.key.as_ref(), b"id", &prefixes)
                            })
                            .and_then(|attribute| {
                                oxoffice_core::xml::attribute_text(&attribute)
                                    .ok()
                                    .and_then(|value| value.parse::<i32>().ok())
                            })
                            .map(|id| properties.special_references.push(id))
                    } else {
                        None
                    };
                    if modeled.is_none() {
                        properties.extra_xml.push((
                            slot,
                            crate::text::raw_with_external_bindings(
                                &capture_empty(child, reader.empty_writing())?,
                                owner_bindings,
                            )?,
                        ));
                    }
                }
                Ok(Event::Start(ref child)) => {
                    properties.extra_xml.push((
                        slot,
                        crate::text::raw_with_external_bindings(
                            &capture_element(reader, child)?,
                            owner_bindings,
                        )?,
                    ));
                }
                Ok(Event::End(ref end)) if matches_local_name(end.name().as_ref(), tag) => break,
                Ok(Event::Eof) => break,
                Err(error) => return Err(error.into()),
                _ => {}
            }
            buf.clear();
        }
        Ok(properties)
    }

    fn parse_cols_attrs(e: &BytesStart, word_prefixes: &[String]) -> Result<CT_Columns> {
        let mut cols = CT_Columns::default();
        for attr in e.attributes() {
            let attr = attr?;
            let key = attr.key.as_ref();
            let val_str = oxoffice_core::xml::attribute_text(&attr)?;
            let val_str: &str = &val_str;
            if is_word_attribute(key, b"num", word_prefixes) {
                cols.num = Some(SectionScalar::count(val_str)?);
            } else if is_word_attribute(key, b"space", word_prefixes) {
                cols.space = Some(Twips(parse_twips_measure(e.name().as_ref(), key, val_str)?));
            } else if is_word_attribute(key, b"equalWidth", word_prefixes) {
                cols.equal_width = Some(ST_OnOff::parse(val_str) == Some(true));
            } else if is_word_attribute(key, b"sep", word_prefixes) {
                cols.sep = Some(ST_OnOff::parse(val_str) == Some(true));
            }
        }
        Ok(cols)
    }

    fn parse_cols_empty(e: &BytesStart, word_prefixes: &[String]) -> Result<CT_Columns> {
        Self::parse_cols_attrs(e, word_prefixes)
    }

    fn parse_cols_start(
        reader: &mut Reader<&[u8]>,
        e: &BytesStart,
        word_prefixes: &[String],
    ) -> Result<CT_Columns> {
        let mut cols = Self::parse_cols_attrs(e, word_prefixes)?;
        let mut buf = Vec::new();

        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Empty(ref e)) => {
                    let prefixes = word_prefixes_at(e, word_prefixes)?;
                    if !is_word_element(e.name().as_ref(), b"col", &prefixes) {
                        buf.clear();
                        continue;
                    }
                    let mut width = None;
                    let mut space = None;
                    for attr in e.attributes() {
                        let attr = attr?;
                        let key = attr.key.as_ref();
                        let value = oxoffice_core::xml::attribute_text(&attr)?;
                        let value: &str = &value;
                        let measurement = || parse_twips_measure(e.name().as_ref(), key, value);
                        if is_word_attribute(key, b"w", &prefixes) {
                            width = Some(Twips(measurement()?));
                        } else if is_word_attribute(key, b"space", &prefixes) {
                            space = Some(Twips(measurement()?));
                        }
                    }
                    cols.columns.push(CT_Column { width, space });
                }
                Ok(Event::End(ref e)) if matches_local_name(e.name().as_ref(), b"cols") => {
                    break;
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(e.into()),
                _ => {}
            }
            buf.clear();
        }

        Ok(cols)
    }

    pub fn to_xml<W: std::io::Write>(&self, writer: &mut Writer<W>) -> Result<()> {
        self.write_xml_role(writer, crate::text::XmlWriteRole::Publication, None)
    }

    pub(crate) fn write_xml_role<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        role: crate::text::XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
    ) -> Result<()> {
        self.write_xml_role_in(writer, role, source_context, None)
    }

    pub(crate) fn write_xml_role_in<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        role: crate::text::XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let mut buf = itoa::Buffer::new();
        let mut root = crate::text::FieldXmlStart::new("w:sectPr", emission)?;
        for (raw, position) in self.extra_xml.iter().zip(&self.extra_xml_positions) {
            if Self::raw_position_is_root_attributes(position, raw) {
                root.root_record(raw, None, role)?;
            }
        }

        // Source composition resolves generated qualifiers from the explicit
        // original consumer frame plus this section's own retained root
        // declarations. Publication keeps canonical names and ambient facts.
        // Retained records are already staged on the root, so the existing
        // qualifier helper overlays them without copying inherited URIs.
        let word_prefix = if role == crate::text::XmlWriteRole::Source {
            std::borrow::Cow::Owned(root.qualify(&[], role, source_context)?)
        } else {
            std::borrow::Cow::Borrowed("w")
        };
        let source_declarations = if role == crate::text::XmlWriteRole::Source {
            Some(crate::text::raw_namespace_declarations(root.element())?)
        } else {
            None
        };
        let child_context = source_declarations.as_ref().map(|declarations| {
            borrowed_source_overlay(source_context.unwrap_or(&[]), declarations)
        });
        let mut relationship_scratch = [0u8; 6 + std::mem::size_of::<usize>() * 3];
        let (relationship_prefix, declare_relationship) =
            if role == crate::text::XmlWriteRole::Source {
                source_relationship_prefix(
                    child_context.as_deref().unwrap_or(&[]),
                    &mut relationship_scratch,
                )
            } else {
                ("r", false)
            };
        root.write(writer, false)?;
        let ordered_raw = self.extra_xml_positions.len() == self.extra_xml.len();
        if ordered_raw {
            self.write_story_reference_boundary(writer, &self.header_refs, true, 0)?;
        }

        // headerReference and footerReference elements
        for (header, index) in self.story_reference_order() {
            let (references, local) = if header {
                (&self.header_refs, "headerReference")
            } else {
                (&self.footer_refs, "footerReference")
            };
            if ordered_raw && !header && index == 0 {
                self.write_story_reference_boundary(writer, references, false, 0)?;
            }
            let reference = &references[index];
            let mut e = crate::text::FieldXmlStart::word(&word_prefix, local, emission)?;
            e.push_word(&word_prefix, "type", reference.hdr_ftr_type.to_str())?;
            if declare_relationship {
                e.push_word("xmlns", relationship_prefix, crate::namespace::R_NS)?;
            }
            e.push_word(relationship_prefix, "id", reference.rel_id.as_str())?;
            e.write(writer, true)?;
            if ordered_raw {
                self.write_story_reference_boundary(writer, references, header, index + 1)?;
            }
        }
        if ordered_raw && self.footer_refs.is_empty() {
            self.write_story_reference_boundary(writer, &self.footer_refs, false, 0)?;
        }
        // footnotePr and endnotePr. Their effect on notes is not applied yet.
        if let Some(footnote_pr) = &self.footnote_pr {
            if role == crate::text::XmlWriteRole::Source {
                footnote_pr.write_xml_role_in(
                    writer,
                    "footnotePr",
                    role,
                    child_context.as_deref(),
                    emission,
                )?;
            } else {
                footnote_pr.write_xml_role_in(
                    writer,
                    "w:footnotePr",
                    role,
                    child_context.as_deref(),
                    emission,
                )?;
            }
        }
        if ordered_raw {
            self.write_raw_position(writer, 2, 0)?;
        }
        if let Some(endnote_pr) = &self.endnote_pr {
            if role == crate::text::XmlWriteRole::Source {
                endnote_pr.write_xml_role_in(
                    writer,
                    "endnotePr",
                    role,
                    child_context.as_deref(),
                    emission,
                )?;
            } else {
                endnote_pr.write_xml_role_in(
                    writer,
                    "w:endnotePr",
                    role,
                    child_context.as_deref(),
                    emission,
                )?;
            }
        }
        if ordered_raw {
            self.write_raw_position(writer, 2, 1)?;
        }

        // type (section break type)
        if let Some(st) = self.section_type {
            let mut e = crate::text::FieldXmlStart::word(&word_prefix, "type", emission)?;
            e.push_word(&word_prefix, "val", st.to_str())?;
            e.write(writer, true)?;
        }
        if ordered_raw {
            self.write_raw_position(writer, 3, 0)?;
        }

        // pgSz
        if self.page_width.is_some() || self.page_height.is_some() || self.orientation.is_some() {
            let mut e = crate::text::FieldXmlStart::word(&word_prefix, "pgSz", emission)?;
            if let Some(w) = self.page_width {
                e.push_word(&word_prefix, "w", buf.format(w.0))?;
            }
            if let Some(h) = self.page_height {
                e.push_word(&word_prefix, "h", buf.format(h.0))?;
            }
            if let Some(orient) = self.orientation
                && orient == ST_PageOrientation::Landscape
            {
                e.push_word(&word_prefix, "orient", orient.to_str())?;
            }
            e.write(writer, true)?;
        }
        if ordered_raw {
            self.write_raw_position(writer, 4, 0)?;
        }

        // pgMar
        if self.margin_top.is_some()
            || self.margin_right.is_some()
            || self.margin_bottom.is_some()
            || self.margin_left.is_some()
            || self.gutter.is_some()
            || self.header_distance.is_some()
            || self.footer_distance.is_some()
        {
            let mut e = crate::text::FieldXmlStart::word(&word_prefix, "pgMar", emission)?;
            if let Some(t) = self.margin_top {
                e.push_word(&word_prefix, "top", buf.format(t.0))?;
            }
            if let Some(r) = self.margin_right {
                e.push_word(&word_prefix, "right", buf.format(r.0))?;
            }
            if let Some(b) = self.margin_bottom {
                e.push_word(&word_prefix, "bottom", buf.format(b.0))?;
            }
            if let Some(l) = self.margin_left {
                e.push_word(&word_prefix, "left", buf.format(l.0))?;
            }
            if let Some(g) = self.gutter {
                e.push_word(&word_prefix, "gutter", buf.format(g.0))?;
            }
            if let Some(h) = self.header_distance {
                e.push_word(&word_prefix, "header", buf.format(h.0))?;
            }
            if let Some(f) = self.footer_distance {
                e.push_word(&word_prefix, "footer", buf.format(f.0))?;
            }
            e.write(writer, true)?;
        }
        if ordered_raw {
            self.write_raw_position(writer, 5, 0)?;
        }

        // paperSrc. A printer tray, so pagination is identical with and without it.
        if let Some(paper_source) = &self.paper_source {
            let mut e = crate::text::FieldXmlStart::new("w:paperSrc", emission)?;
            for (name, value) in &paper_source.extra_attributes {
                e.push_raw(name, value)?;
            }
            let word_prefix = if role == crate::text::XmlWriteRole::Source {
                std::borrow::Cow::Owned(e.qualify(&[], role, child_context.as_deref())?)
            } else {
                word_prefix.clone()
            };
            if let Some(first) = paper_source.first {
                e.push_word(&word_prefix, "first", buf.format(first))?;
            }
            if let Some(other) = paper_source.other {
                e.push_word(&word_prefix, "other", buf.format(other))?;
            }
            e.write(writer, true)?;
        }
        if ordered_raw {
            self.write_raw_position(writer, 5, 1)?;
        }

        // pgBorders
        if let Some(page_borders) = &self.page_borders {
            page_borders.write_xml_role_in(writer, role, child_context.as_deref(), emission)?;
        }
        if ordered_raw {
            self.write_raw_position(writer, 5, 2)?;
        }

        // lnNumType
        if let Some(line_numbers) = &self.line_numbers {
            let mut e = crate::text::FieldXmlStart::new("w:lnNumType", emission)?;
            for (name, value) in &line_numbers.extra_attributes {
                e.push_raw(name, value)?;
            }
            let word_prefix = if role == crate::text::XmlWriteRole::Source {
                std::borrow::Cow::Owned(e.qualify(&[], role, child_context.as_deref())?)
            } else {
                word_prefix.clone()
            };
            if let Some(count_by) = line_numbers.count_by {
                e.push_word(&word_prefix, "countBy", buf.format(count_by))?;
            }
            if let Some(start) = line_numbers.start {
                e.push_word(&word_prefix, "start", buf.format(start))?;
            }
            if let Some(distance) = line_numbers.distance {
                e.push_word(&word_prefix, "distance", buf.format(distance.0))?;
            }
            if let Some(restart) = line_numbers.restart {
                e.push_word(&word_prefix, "restart", restart.to_str())?;
            }
            e.write(writer, true)?;
        }
        if ordered_raw {
            self.write_raw_position(writer, 5, 3)?;
        }

        // pgNumType. M24 format and chapter attributes stay byte-exact unless start changes.
        if let Some(page_number) = &self.page_number {
            page_number.write_xml_role_in(writer, role, child_context.as_deref(), emission)?;
        }
        if ordered_raw {
            self.write_raw_position(writer, 6, 1)?;
        }

        // cols
        if let Some(ref cols) = self.columns {
            if cols.columns.is_empty() {
                // Simple equal-width columns
                let mut e = crate::text::FieldXmlStart::word(&word_prefix, "cols", emission)?;
                if let Some(num) = cols.num {
                    e.push_word(&word_prefix, "num", buf.format(num))?;
                }
                if let Some(space) = cols.space {
                    e.push_word(&word_prefix, "space", buf.format(space.0))?;
                }
                if let Some(eq) = cols.equal_width
                    && !eq
                {
                    e.push_word(&word_prefix, "equalWidth", "0")?;
                }
                if let Some(sep) = cols.sep
                    && sep
                {
                    e.push_word(&word_prefix, "sep", "1")?;
                }
                e.write(writer, true)?;
            } else {
                // Individual column definitions
                let mut e = crate::text::FieldXmlStart::word(&word_prefix, "cols", emission)?;
                if let Some(num) = cols.num {
                    e.push_word(&word_prefix, "num", buf.format(num))?;
                }
                if let Some(eq) = cols.equal_width {
                    e.push_word(&word_prefix, "equalWidth", if eq { "1" } else { "0" })?;
                }
                if let Some(sep) = cols.sep
                    && sep
                {
                    e.push_word(&word_prefix, "sep", "1")?;
                }
                e.write(writer, false)?;

                for col in &cols.columns {
                    let mut ce = crate::text::FieldXmlStart::word(&word_prefix, "col", emission)?;
                    if let Some(w) = col.width {
                        ce.push_word(&word_prefix, "w", buf.format(w.0))?;
                    }
                    if let Some(s) = col.space {
                        ce.push_word(&word_prefix, "space", buf.format(s.0))?;
                    }
                    ce.write(writer, true)?;
                }

                if let Some(emission) = emission {
                    emission.write_word_end(writer, &word_prefix, "cols")?;
                } else {
                    writer.write_event(Event::End(BytesEnd::new(format!("{word_prefix}:cols"))))?;
                }
            }
        }
        if ordered_raw {
            self.write_raw_position(writer, 7, 0)?;
        }

        // vAlign
        if let Some(vertical_alignment) = self.vertical_alignment {
            let mut e = crate::text::FieldXmlStart::word(&word_prefix, "vAlign", emission)?;
            e.push_word(&word_prefix, "val", vertical_alignment.to_str())?;
            e.write(writer, true)?;
        }
        if ordered_raw {
            self.write_raw_position(writer, 7, 1)?;
        }

        // titlePg
        if let Some(title_pg) = self.title_pg {
            let mut e = crate::text::FieldXmlStart::word(&word_prefix, "titlePg", emission)?;
            if !title_pg {
                e.push_word(&word_prefix, "val", "0")?;
            }
            e.write(writer, true)?;
        }

        if ordered_raw {
            self.write_raw_position(writer, 8, 0)?;
        }

        // textDirection. Authored and preserved here, projected by layout.
        if let Some(text_direction) = &self.text_direction {
            let mut e = crate::text::FieldXmlStart::word(&word_prefix, "textDirection", emission)?;
            e.push_word(&word_prefix, "val", text_direction.as_str())?;
            e.write(writer, true)?;
        }

        // docGrid. The grid type, line pitch and character space this crate
        // models, with any producer attribute it does not written first. A
        // legacy caller that populates only `extra_xml` gets the modeled
        // element ahead of its raw dump, which is the order the rest of this
        // serialiser already writes a modeled child in.
        let mut write_doc_grid = |writer: &mut Writer<W>| -> Result<()> {
            if let Some(doc_grid) = &self.doc_grid {
                let mut e = crate::text::FieldXmlStart::new("w:docGrid", emission)?;
                for (name, value) in &doc_grid.extra_attributes {
                    e.push_raw(name, value)?;
                }
                let word_prefix = if role == crate::text::XmlWriteRole::Source {
                    std::borrow::Cow::Owned(e.qualify(&[], role, child_context.as_deref())?)
                } else {
                    word_prefix.clone()
                };
                if let Some(grid_type) = doc_grid.grid_type {
                    e.push_word(&word_prefix, "type", grid_type.to_str())?;
                }
                if let Some(line_pitch) = doc_grid.line_pitch {
                    e.push_word(&word_prefix, "linePitch", buf.format(line_pitch.0))?;
                }
                if let Some(char_space) = doc_grid.char_space {
                    e.push_word(&word_prefix, "charSpace", buf.format(char_space))?;
                }
                e.write(writer, true)?;
            }
            Ok(())
        };
        if ordered_raw {
            self.write_raw_position(writer, 8, 1)?;
            write_doc_grid(writer)?;
            self.write_raw_position(writer, 8, 2)?;
            self.write_raw_position(writer, 9, 0)?;
        } else {
            write_doc_grid(writer)?;
            // Legacy callers that populate only `extra_xml` retain the previous position.
            for raw in &self.extra_xml {
                if is_root_attribute_record(raw) {
                    continue;
                }
                writer.get_mut().write_all(raw)?;
            }
        }
        if let Some(change) = &self.change {
            change.write_xml_with_role_override(
                writer,
                None,
                role,
                child_context.as_deref(),
                emission,
            )?;
        }

        if let Some(emission) = emission {
            emission.write_word_end(writer, &word_prefix, "sectPr")?;
        } else {
            writer.write_event(Event::End(BytesEnd::new(format!("{word_prefix}:sectPr"))))?;
        }
        Ok(())
    }

    fn write_raw_position<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        slot: usize,
        occurrence: usize,
    ) -> Result<()> {
        for (position, raw) in self.extra_xml_positions.iter().zip(&self.extra_xml) {
            if matches!(
                position,
                CT_SectPrRawPosition::Schema {
                    slot: candidate_slot,
                    occurrence: candidate_occurrence,
                } if (*candidate_slot, *candidate_occurrence) == (slot, occurrence)
            ) {
                writer.get_mut().write_all(raw)?;
            }
        }
        Ok(())
    }

    fn write_story_reference_boundary<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        references: &[HdrFtrRef],
        header: bool,
        boundary: usize,
    ) -> Result<()> {
        for (position, raw) in self.extra_xml_positions.iter().zip(&self.extra_xml) {
            let anchored = match position {
                CT_SectPrRawPosition::Header {
                    preceding,
                    following,
                } if header => Some((preceding, following)),
                CT_SectPrRawPosition::Footer {
                    preceding,
                    following,
                } if !header => Some((preceding, following)),
                CT_SectPrRawPosition::Schema { slot, occurrence }
                    if (*slot == 0 && header) || (*slot == 1 && !header) =>
                {
                    if (*occurrence).min(references.len()) == boundary {
                        writer.get_mut().write_all(raw)?;
                    }
                    None
                }
                _ => None,
            };
            let Some((preceding, following)) = anchored else {
                continue;
            };
            let resolved = following
                .as_ref()
                .and_then(|reference| reference.resolve(references))
                .or_else(|| {
                    preceding
                        .as_ref()
                        .and_then(|reference| reference.resolve(references).map(|index| index + 1))
                })
                .unwrap_or(references.len());
            if resolved == boundary {
                writer.get_mut().write_all(raw)?;
            }
        }
        Ok(())
    }
}

impl CT_PageBorders {
    /// Write `w:pgBorders` with its edges in `xsd:sequence` order.
    ///
    /// Retained attributes are written ahead of the modeled ones and retained
    /// children after the modeled edges, which is what `CT_BorderEdge` does.
    fn write_xml_role_in<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        role: crate::text::XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let mut e = crate::text::FieldXmlStart::new("w:pgBorders", emission)?;
        for (name, value) in &self.extra_attributes {
            e.push_raw(name, value)?;
        }
        let word_prefix = if role == crate::text::XmlWriteRole::Source {
            std::borrow::Cow::Owned(e.qualify(&[], role, source_context)?)
        } else {
            std::borrow::Cow::Borrowed("w")
        };
        let page_declarations = if role == crate::text::XmlWriteRole::Source {
            Some(crate::text::raw_namespace_declarations(e.element())?)
        } else {
            None
        };
        let edge_context = page_declarations.as_ref().map(|declarations| {
            borrowed_source_overlay(source_context.unwrap_or(&[]), declarations)
        });
        if let Some(z_order) = self.z_order {
            e.push_word(&word_prefix, "zOrder", z_order.to_str())?;
        }
        if let Some(display) = self.display {
            e.push_word(&word_prefix, "display", display.to_str())?;
        }
        if let Some(offset_from) = self.offset_from {
            e.push_word(&word_prefix, "offsetFrom", offset_from.to_str())?;
        }
        if self.top.is_none()
            && self.left.is_none()
            && self.bottom.is_none()
            && self.right.is_none()
            && self.extra_xml.is_empty()
        {
            e.write(writer, true)?;
            return Ok(());
        }
        e.write(writer, false)?;
        for (edge, local, qualified) in [
            (&self.top, "top", "w:top"),
            (&self.left, "left", "w:left"),
            (&self.bottom, "bottom", "w:bottom"),
            (&self.right, "right", "w:right"),
        ] {
            if let Some(edge) = edge {
                if role == crate::text::XmlWriteRole::Source {
                    edge.write_xml_in(writer, local, role, edge_context.as_deref(), emission)?;
                } else {
                    edge.write_xml_in(writer, qualified, role, edge_context.as_deref(), emission)?;
                }
            }
        }
        for raw in &self.extra_xml {
            writer.get_mut().write_all(raw)?;
        }
        if let Some(emission) = emission {
            emission.write_word_end(writer, &word_prefix, "pgBorders")?;
        } else {
            writer.write_event(Event::End(BytesEnd::new(format!(
                "{word_prefix}:pgBorders"
            ))))?;
        }
        Ok(())
    }
}

impl CT_NoteProperties {
    /// Write section note properties under `tag`, in `xsd:sequence` order.
    pub(crate) fn to_xml<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        tag: &str,
    ) -> Result<()> {
        self.write_xml_role_in(
            writer,
            tag,
            crate::text::XmlWriteRole::Publication,
            None,
            None,
        )
    }

    fn write_xml_role_in<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        name: &str,
        role: crate::text::XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        let source = role == crate::text::XmlWriteRole::Source;
        let local = if source {
            name
        } else {
            name.rsplit(':').next().unwrap_or(name)
        };
        let mut start = if source {
            crate::text::FieldXmlStart::word("w", name, emission)?
        } else {
            crate::text::FieldXmlStart::new(name, emission)?
        };
        let word_prefix = if source {
            std::borrow::Cow::Owned(start.qualify(&[], role, source_context)?)
        } else {
            std::borrow::Cow::Borrowed("w")
        };
        self.write_xml_core(
            writer,
            start,
            &word_prefix,
            local,
            (!source).then_some(name),
            emission,
        )
    }

    fn write_xml_core<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        start: crate::text::FieldXmlStart<'_, '_>,
        word_prefix: &str,
        local: &str,
        publication_end: Option<&str>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        if self.pos.is_none()
            && self.num_fmt.is_none()
            && self.num_start.is_none()
            && self.num_restart.is_none()
            && self.special_references.is_empty()
            && self.extra_xml.is_empty()
        {
            start.write(writer, true)?;
            return Ok(());
        }
        start.write(writer, false)?;
        self.write_retained_children(writer, 0)?;
        if let Some(pos) = &self.pos {
            let mut e = crate::text::FieldXmlStart::word(word_prefix, "pos", emission)?;
            e.push_word(word_prefix, "val", pos.as_str())?;
            e.write(writer, true)?;
        }
        self.write_retained_children(writer, 1)?;
        if let Some(num_fmt) = &self.num_fmt {
            let mut e = crate::text::FieldXmlStart::word(word_prefix, "numFmt", emission)?;
            e.push_word(word_prefix, "val", num_fmt.as_str())?;
            e.write(writer, true)?;
        }
        self.write_retained_children(writer, 2)?;
        if let Some(num_start) = self.num_start {
            let mut buf = itoa::Buffer::new();
            let mut e = crate::text::FieldXmlStart::word(word_prefix, "numStart", emission)?;
            e.push_word(word_prefix, "val", buf.format(num_start))?;
            e.write(writer, true)?;
        }
        self.write_retained_children(writer, 3)?;
        if let Some(num_restart) = &self.num_restart {
            let mut e = crate::text::FieldXmlStart::word(word_prefix, "numRestart", emission)?;
            e.push_word(word_prefix, "val", num_restart.as_str())?;
            e.write(writer, true)?;
        }
        self.write_retained_children(writer, 4)?;
        let child_local = if local.ends_with("footnotePr") {
            "footnote"
        } else {
            "endnote"
        };
        for id in &self.special_references {
            let mut buf = itoa::Buffer::new();
            let mut child = crate::text::FieldXmlStart::word(word_prefix, child_local, emission)?;
            child.push_word(word_prefix, "id", buf.format(*id))?;
            child.write(writer, true)?;
        }
        self.write_retained_children(writer, 5)?;
        if let Some(name) = publication_end {
            if let Some(emission) = emission {
                emission.write_end_name(writer, name)?;
            } else {
                writer.write_event(Event::End(BytesEnd::new(name)))?;
            }
        } else if let Some(emission) = emission {
            emission.write_word_end(writer, word_prefix, local)?;
        } else {
            writer.write_event(Event::End(BytesEnd::new(format!("{word_prefix}:{local}"))))?;
        }
        Ok(())
    }

    fn write_retained_children<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        slot: usize,
    ) -> Result<()> {
        for (candidate, raw) in &self.extra_xml {
            if *candidate == slot {
                writer.get_mut().write_all(raw)?;
            }
        }
        Ok(())
    }
}

impl CT_PageNumberType {
    fn write_xml_role_in<W: std::io::Write>(
        &self,
        writer: &mut Writer<W>,
        role: crate::text::XmlWriteRole,
        source_context: Option<&[(&str, &str)]>,
        emission: Option<&crate::text::FieldXmlEmission>,
    ) -> Result<()> {
        if let Some(raw) = &self.raw_xml {
            if self.start == self.parsed_start {
                writer.get_mut().write_all(raw)?;
                return Ok(());
            }
            let produced = replace_page_number_start_in(
                raw,
                self.start,
                self.start_attribute_name.as_deref(),
                self.start_insertion_prefix.as_deref(),
                &self.reserved_insertion_prefixes,
                emission,
            )?;
            let _transport = emission
                .map(|emission| emission.prepaid_xml_transport(&produced))
                .transpose()?;
            writer.get_mut().write_all(&produced)?;
            return Ok(());
        }

        let mut element = crate::text::FieldXmlStart::new("w:pgNumType", emission)?;
        let word_prefix = if role == crate::text::XmlWriteRole::Source {
            std::borrow::Cow::Owned(element.qualify(&[], role, source_context)?)
        } else {
            std::borrow::Cow::Borrowed("w")
        };
        if let Some(start) = self.start {
            let mut buf = itoa::Buffer::new();
            element.push_word(&word_prefix, "start", buf.format(start))?;
        }
        element.write(writer, true)?;
        Ok(())
    }
}

/// Store a parsed unsigned attribute, or retain it verbatim when the source
/// value falls outside what the field models, so nothing is dropped on write.
/// Stores `value`, `attribute` read, in `field`, or keeps `attribute` as
/// written in `retained`, which is written back raw.
fn store_unsigned_attribute(
    value: &str,
    attribute: &quick_xml::events::attributes::Attribute<'_>,
    field: &mut Option<u32>,
    retained: &mut Vec<(String, String)>,
) -> Result<()> {
    match value.parse() {
        Ok(parsed) => *field = Some(parsed),
        Err(_) => retained.push((
            std::str::from_utf8(attribute.key.as_ref())?.to_owned(),
            std::str::from_utf8(&attribute.value)?.to_owned(),
        )),
    }
    Ok(())
}

/// Return the `w:val` of an element whose only attribute is `w:val`.
///
/// `None` means the element carries something this crate does not model, and
/// the caller keeps the element raw rather than typing part of it.
fn sole_val_attribute(e: &BytesStart<'_>, word_prefixes: &[String]) -> Result<Option<String>> {
    let mut value = None;
    for attribute in e.attributes() {
        let attribute = attribute?;
        if !is_word_attribute(attribute.key.as_ref(), b"val", word_prefixes) {
            return Ok(None);
        }
        value = Some(
            attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, e.decoder())?
                .into_owned(),
        );
    }
    Ok(value)
}

/// Overlay owned physical declarations on a borrowed consumer frame without
/// copying inherited prefix or URI strings. Later local entries shadow earlier
/// base entries while preserving their lexical order.
fn borrowed_source_overlay<'a>(
    base: &'a [(&'a str, &'a str)],
    local: &'a [(String, String)],
) -> Vec<(&'a str, &'a str)> {
    let mut overlay = base
        .iter()
        .copied()
        .filter(|(prefix, _)| !local.iter().any(|(known, _)| known == prefix))
        .collect::<Vec<_>>();
    overlay.extend(
        local
            .iter()
            .map(|(prefix, namespace)| (prefix.as_str(), namespace.as_str())),
    );
    overlay
}

/// Resolve a source relationship qualifier without rebinding a local `r`
/// shadow. An existing relationship alias wins, then the free preferred
/// prefix, then a deterministic bounded `rdocxR` alias declared by the caller.
fn source_relationship_prefix<'a>(
    bindings: &'a [(&str, &str)],
    scratch: &'a mut [u8; 6 + std::mem::size_of::<usize>() * 3],
) -> (&'a str, bool) {
    let relationships = crate::namespace::R_NS;
    if bindings
        .iter()
        .any(|(prefix, namespace)| *prefix == "r" && *namespace == relationships)
    {
        return ("r", false);
    }
    if let Some((prefix, _)) = bindings
        .iter()
        .find(|(prefix, namespace)| !prefix.is_empty() && *namespace == relationships)
    {
        return (*prefix, false);
    }
    if !bindings.iter().any(|(prefix, _)| *prefix == "r") {
        return ("r", true);
    }
    let mut suffix = 0usize;
    let mut decimal = itoa::Buffer::new();
    loop {
        let digits = if suffix == 0 {
            ""
        } else {
            decimal.format(suffix)
        };
        let occupied = bindings.iter().any(|(prefix, _)| {
            prefix.as_bytes().strip_prefix(b"rdocxR") == Some(digits.as_bytes())
        });
        if !occupied {
            scratch[..6].copy_from_slice(b"rdocxR");
            scratch[6..6 + digits.len()].copy_from_slice(digits.as_bytes());
            return (
                std::str::from_utf8(&scratch[..6 + digits.len()])
                    .expect("ASCII relationship prefix"),
                true,
            );
        }
        suffix += 1;
    }
}

fn replace_page_number_start_in(
    raw: &[u8],
    start: Option<u32>,
    attribute_name: Option<&[u8]>,
    insertion_prefix: Option<&[u8]>,
    reserved_prefixes: &[Vec<u8>],
    emission: Option<&crate::text::FieldXmlEmission>,
) -> Result<Vec<u8>> {
    let mut decimal = itoa::Buffer::new();
    let replacement = start.map(|value| decimal.format(value) as &str);
    let Some((tag_end, attributes)) = start_tag_attributes(raw) else {
        if let Some(emission) = emission {
            emission.reserve_inner_xml(raw.len())?;
        }
        return Ok(raw.to_vec());
    };
    let overflow = || OxmlError::InvalidValue("page number source output overflow".into());
    if let Some(name) = attribute_name
        && let Some((_, value, full)) = attributes
            .iter()
            .find(|(candidate, _, _)| &raw[candidate.clone()] == name)
    {
        let range = if replacement.is_some() { value } else { full };
        let length = raw
            .len()
            .checked_sub(range.len())
            .and_then(|n| n.checked_add(replacement.map_or(0, str::len)))
            .ok_or_else(overflow)?;
        if let Some(emission) = emission {
            emission.reserve_inner_xml(length)?;
        }
        let mut output = Vec::with_capacity(length);
        output.extend_from_slice(&raw[..range.start]);
        if let Some(value) = replacement {
            output.extend_from_slice(value.as_bytes());
        }
        output.extend_from_slice(&raw[range.end..]);
        return Ok(output);
    }
    let Some(value) = replacement else {
        if let Some(emission) = emission {
            emission.reserve_inner_xml(raw.len())?;
        }
        return Ok(raw.to_vec());
    };
    let insert_at = raw[..tag_end]
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace() && *byte != b'/')
        .map_or(tag_end, |index| index + 1);
    let mut prefix_scratch = [0u8; 9 + std::mem::size_of::<usize>() * 3];
    let mut suffix_decimal = itoa::Buffer::new();
    let prefix = if let Some(prefix) = insertion_prefix {
        prefix
    } else {
        let index = unused_page_number_prefix_index(raw, tag_end, &attributes, reserved_prefixes);
        let digits = if index == 0 {
            ""
        } else {
            suffix_decimal.format(index)
        };
        prefix_scratch[..9].copy_from_slice(b"rdocxWord");
        prefix_scratch[9..9 + digits.len()].copy_from_slice(digits.as_bytes());
        &prefix_scratch[..9 + digits.len()]
    };
    // This is exactly String::from_utf8_lossy's public primitive, without a
    // temporary lossy prefix String before the actual output reservation.
    let prefix_len = prefix
        .utf8_chunks()
        .try_fold(0usize, |sum, chunk| {
            sum.checked_add(chunk.valid().len())
                .and_then(|n| n.checked_add(if chunk.invalid().is_empty() { 0 } else { 3 }))
        })
        .ok_or_else(overflow)?;
    let mut added = 10usize
        .checked_add(prefix_len)
        .and_then(|n| n.checked_add(value.len()))
        .ok_or_else(overflow)?;
    if insertion_prefix.is_none() {
        added = added
            .checked_add(10)
            .and_then(|n| n.checked_add(prefix_len))
            .and_then(|n| n.checked_add(W_NS.len()))
            .ok_or_else(overflow)?;
    }
    let length = raw.len().checked_add(added).ok_or_else(overflow)?;
    if let Some(emission) = emission {
        emission.reserve_inner_xml(length)?;
    }
    let mut output = Vec::with_capacity(length);
    output.extend_from_slice(&raw[..insert_at]);
    if insertion_prefix.is_none() {
        output.extend_from_slice(b" xmlns:");
        append_lossy_prefix(&mut output, prefix);
        output.extend_from_slice(b"=\"");
        output.extend_from_slice(W_NS.as_bytes());
        output.push(b'"');
    }
    output.push(b' ');
    append_lossy_prefix(&mut output, prefix);
    output.extend_from_slice(b":start=\"");
    output.extend_from_slice(value.as_bytes());
    output.push(b'"');
    output.extend_from_slice(&raw[insert_at..]);
    debug_assert_eq!(output.len(), length);
    Ok(output)
}

fn append_lossy_prefix(output: &mut Vec<u8>, prefix: &[u8]) {
    for chunk in prefix.utf8_chunks() {
        output.extend_from_slice(chunk.valid().as_bytes());
        if !chunk.invalid().is_empty() {
            output.extend_from_slice("�".as_bytes());
        }
    }
}

fn unused_page_number_prefix_index(
    raw: &[u8],
    tag_end: usize,
    attributes: &[AttributeSpans],
    reserved_prefixes: &[Vec<u8>],
) -> usize {
    let mut decimal = itoa::Buffer::new();
    for suffix in 0usize.. {
        let digits = if suffix == 0 {
            ""
        } else {
            decimal.format(suffix)
        };
        let exact = |name: &[u8]| name.strip_prefix(b"rdocxWord") == Some(digits.as_bytes());
        let qualified = |name: &[u8]| {
            name.strip_prefix(b"rdocxWord")
                .and_then(|tail| tail.strip_prefix(digits.as_bytes()))
                .is_some_and(|tail| tail.starts_with(b":"))
        };
        let used = reserved_prefixes.iter().any(|candidate| exact(candidate))
            || attributes.iter().any(|(name, _, _)| {
                let name = &raw[name.clone()];
                name.strip_prefix(b"xmlns:").is_some_and(exact) || qualified(name)
            })
            || qualified(&raw[1..tag_end]);
        if !used {
            return suffix;
        }
    }
    unreachable!("the finite start tag cannot use every generated prefix")
}

fn attribute_in_namespace(
    name: &[u8],
    local_name: &[u8],
    namespace: &str,
    scope: &[String],
) -> bool {
    let Some(separator) = name.iter().position(|byte| *byte == b':') else {
        return false;
    };
    if name.get(separator + 1..) != Some(local_name) {
        return false;
    }
    let prefix = &name[..separator];
    scope.iter().any(|binding| {
        binding
            .strip_prefix('\0')
            .and_then(|binding| binding.split_once('\0'))
            .is_some_and(|(candidate, value)| candidate.as_bytes() == prefix && value == namespace)
    })
}

fn page_size_attributes_are_modeled(
    element: &BytesStart<'_>,
    word_prefixes: &[String],
) -> Result<bool> {
    for attribute in element.attributes() {
        let attribute = attribute?;
        let key = attribute.key.as_ref();
        if namespace_declaration(key) {
            continue;
        }
        let value = oxoffice_core::xml::attribute_text(&attribute)?;
        let value: &str = &value;
        if is_word_attribute(key, b"w", word_prefixes)
            || is_word_attribute(key, b"h", word_prefixes)
        {
            if parse_twips_measure::<i32>(element.name().as_ref(), key, value).is_err() {
                return Ok(false);
            }
        } else if is_word_attribute(key, b"orient", word_prefixes) {
            if ST_PageOrientation::from_str(value).is_err() {
                return Ok(false);
            }
        } else {
            return Ok(false);
        }
    }
    Ok(true)
}

fn page_margin_attributes_are_modeled(
    element: &BytesStart<'_>,
    word_prefixes: &[String],
) -> Result<bool> {
    for attribute in element.attributes() {
        let attribute = attribute?;
        let key = attribute.key.as_ref();
        if namespace_declaration(key) {
            continue;
        }
        let modeled = [
            b"top".as_slice(),
            b"right".as_slice(),
            b"end".as_slice(),
            b"bottom".as_slice(),
            b"left".as_slice(),
            b"start".as_slice(),
            b"gutter".as_slice(),
            b"header".as_slice(),
            b"footer".as_slice(),
        ]
        .iter()
        .any(|local| is_word_attribute(key, local, word_prefixes));
        let value = oxoffice_core::xml::attribute_text(&attribute)?;
        let value: &str = &value;
        if !modeled || parse_twips_measure::<i32>(element.name().as_ref(), key, value).is_err() {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The modeled section sequence used by the owning serializer.
pub(crate) fn checked_section_child_slot(local: &[u8]) -> Option<(u8, bool)> {
    let slot = match local {
        b"headerReference" => return Some((0, true)),
        b"footerReference" => return Some((1, true)),
        b"footnotePr" => 2,
        b"endnotePr" => 3,
        b"type" => 4,
        b"pgSz" => 5,
        b"pgMar" => 6,
        b"paperSrc" => 7,
        b"pgBorders" => 8,
        b"lnNumType" => 9,
        b"pgNumType" => 10,
        b"cols" => 11,
        b"vAlign" => 12,
        b"titlePg" => 13,
        b"textDirection" => 14,
        b"docGrid" => 15,
        b"sectPrChange" => 16,
        _ => return None,
    };
    Some((slot, false))
}

pub(crate) fn validate_checked_section_child(
    element: &BytesStart<'_>,
    prefixes: &[String],
) -> Result<bool> {
    let name = element.local_name();
    let local = name.as_ref();
    if matches!(
        local,
        b"p" | b"r" | b"tbl" | b"tr" | b"tc" | b"pPr" | b"rPr" | b"sectPr"
    ) {
        return Err(OxmlError::InvalidValue(
            "element XML section has a known child with the wrong owner".into(),
        ));
    }
    if matches!(local, b"headerReference" | b"footerReference") {
        if !story_reference_attributes_are_modeled(element, prefixes)? {
            return Err(OxmlError::InvalidValue(
                "element XML section reference has unsupported or invalid owner attributes".into(),
            ));
        }
        return Ok(true);
    }
    for attribute in element.attributes() {
        let attribute = attribute?;
        let key = attribute.key.as_ref();
        let value =
            attribute.decoded_and_normalized_value(XmlVersion::Implicit1_0, element.decoder())?;
        let invalid = match local {
            b"type" if is_word_attribute(key, b"val", prefixes) => {
                ST_SectionType::from_str(&value).is_err()
            }
            b"pgSz" if is_word_attribute(key, b"orient", prefixes) => {
                ST_PageOrientation::from_str(&value).is_err()
            }
            b"titlePg" if is_word_attribute(key, b"val", prefixes) => {
                crate::shared::ST_OnOff::parse(&value).is_none()
            }
            b"vAlign" if is_word_attribute(key, b"val", prefixes) => {
                !matches!(value.as_ref(), "top" | "center" | "bottom" | "both")
            }
            b"pgSz"
                if is_word_attribute(key, b"w", prefixes)
                    || is_word_attribute(key, b"h", prefixes) =>
            {
                parse_twips_measure::<i32>(element.name().as_ref(), key, &value).is_err()
            }
            _ => false,
        };
        if invalid {
            return Err(OxmlError::InvalidValue(
                "element XML section property has an invalid modeled value".into(),
            ));
        }
    }
    Ok(matches!(
        local,
        b"type"
            | b"pgSz"
            | b"pgMar"
            | b"paperSrc"
            | b"lnNumType"
            | b"pgNumType"
            | b"vAlign"
            | b"titlePg"
            | b"textDirection"
            | b"docGrid"
    ))
}

fn story_reference_attributes_are_modeled(
    element: &BytesStart<'_>,
    word_prefixes: &[String],
) -> Result<bool> {
    let mut type_count = 0usize;
    let mut relationship_count = 0usize;
    for attribute in element.attributes() {
        let attribute = attribute?;
        let key = attribute.key.as_ref();
        if namespace_declaration(key) {
            continue;
        }
        let value = oxoffice_core::xml::attribute_text(&attribute)?;
        let value: &str = &value;
        if is_word_attribute(key, b"type", word_prefixes) {
            type_count += 1;
            if type_count > 1 || !matches!(value, "default" | "first" | "even") {
                return Ok(false);
            }
        } else if attribute_in_namespace(key, b"id", crate::namespace::R_NS, word_prefixes) {
            relationship_count += 1;
            if relationship_count > 1 || value.is_empty() {
                return Ok(false);
            }
        } else {
            return Ok(false);
        }
    }
    Ok(relationship_count == 1)
}

fn namespace_declaration(name: &[u8]) -> bool {
    name == b"xmlns" || name.starts_with(b"xmlns:")
}

fn raw_element_has_child_content(raw: &[u8]) -> Result<bool> {
    let mut reader = Reader::from_reader(raw);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(_) => {
                if depth > 0 {
                    return Ok(true);
                }
                depth += 1;
            }
            Event::Empty(_) if depth > 0 => return Ok(true),
            Event::Text(text)
                if depth > 0 && text.as_ref().iter().any(|byte| !byte.is_ascii_whitespace()) =>
            {
                return Ok(true);
            }
            Event::CData(text)
                if depth > 0 && text.as_ref().iter().any(|byte| !byte.is_ascii_whitespace()) =>
            {
                return Ok(true);
            }
            Event::GeneralRef(_) if depth > 0 => return Ok(true),
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Eof => return Ok(false),
            _ => {}
        }
        buffer.clear();
    }
}

type AttributeSpans = (
    std::ops::Range<usize>,
    std::ops::Range<usize>,
    std::ops::Range<usize>,
);

fn start_tag_attributes(raw: &[u8]) -> Option<(usize, Vec<AttributeSpans>)> {
    if raw.first() != Some(&b'<') {
        return None;
    }
    let mut quote = None;
    let tag_end = raw
        .iter()
        .enumerate()
        .find_map(|(index, byte)| match (*byte, quote) {
            (b'\'' | b'"', None) => {
                quote = Some(*byte);
                None
            }
            (value, Some(open)) if value == open => {
                quote = None;
                None
            }
            (b'>', None) => Some(index),
            _ => None,
        })?;

    let mut cursor = 1usize;
    while cursor < tag_end && !raw[cursor].is_ascii_whitespace() && raw[cursor] != b'/' {
        cursor += 1;
    }
    let mut attributes = Vec::new();
    while cursor < tag_end {
        let full_start = cursor;
        while cursor < tag_end && raw[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor == tag_end || raw[cursor] == b'/' {
            break;
        }
        let name_start = cursor;
        while cursor < tag_end
            && !raw[cursor].is_ascii_whitespace()
            && raw[cursor] != b'='
            && raw[cursor] != b'/'
        {
            cursor += 1;
        }
        let name_end = cursor;
        while cursor < tag_end && raw[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if raw.get(cursor) != Some(&b'=') {
            return None;
        }
        cursor += 1;
        while cursor < tag_end && raw[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let open_quote = *raw.get(cursor)?;
        if open_quote != b'\'' && open_quote != b'"' {
            return None;
        }
        cursor += 1;
        let value_start = cursor;
        while cursor < tag_end && raw[cursor] != open_quote {
            cursor += 1;
        }
        if cursor == tag_end {
            return None;
        }
        let value_end = cursor;
        cursor += 1;
        attributes.push((
            name_start..name_end,
            value_start..value_end,
            full_start..cursor,
        ));
    }
    Some((tag_end, attributes))
}

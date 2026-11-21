//! List and numbering values: number formats, levels, definitions, instances and their conversions to and from the model.

use super::*;

/// Numbering format for one level of a custom list definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListNumberFormat {
    Decimal,
    UpperRoman,
    LowerRoman,
    UpperLetter,
    LowerLetter,
    Ordinal,
    CardinalText,
    OrdinalText,
    Hex,
    Chicago,
    IdeographDigital,
    JapaneseCounting,
    Aiueo,
    Iroha,
    DecimalFullWidth,
    DecimalHalfWidth,
    JapaneseLegal,
    JapaneseDigitalTenThousand,
    DecimalEnclosedCircle,
    DecimalFullWidth2,
    AiueoFullWidth,
    IrohaFullWidth,
    DecimalZero,
    Bullet,
    Ganada,
    Chosung,
    DecimalEnclosedFullstop,
    DecimalEnclosedParen,
    DecimalEnclosedCircleChinese,
    IdeographEnclosedCircle,
    IdeographTraditional,
    IdeographZodiac,
    IdeographZodiacTraditional,
    TaiwaneseCounting,
    IdeographLegalTraditional,
    TaiwaneseCountingThousand,
    TaiwaneseDigital,
    ChineseCounting,
    ChineseLegalSimplified,
    ChineseCountingThousand,
    KoreanDigital,
    KoreanCounting,
    KoreanLegal,
    KoreanDigital2,
    Hebrew1,
    ArabicAlpha,
    Hebrew2,
    ArabicAbjad,
    HindiVowels,
    HindiConsonants,
    HindiNumbers,
    HindiCounting,
    ThaiLetters,
    ThaiNumbers,
    ThaiCounting,
    VietnameseCounting,
    NumberInDash,
    RussianLower,
    RussianUpper,
    None,
    /// A producer-defined format retained across inspection and mutation.
    Other(String),
}

impl ListNumberFormat {
    /// The format an OOXML `w:numFmt` name selects, such as `decimal` or
    /// `lowerLetter`. A name outside the standard set is `Other`.
    pub fn from_name(name: &str) -> Self {
        list_number_format_from_st(ST_NumberFormat::from_str(name))
    }

    pub(super) fn to_st(&self) -> ST_NumberFormat {
        match self {
            Self::Decimal => ST_NumberFormat::Decimal,
            Self::UpperRoman => ST_NumberFormat::UpperRoman,
            Self::LowerRoman => ST_NumberFormat::LowerRoman,
            Self::UpperLetter => ST_NumberFormat::UpperLetter,
            Self::LowerLetter => ST_NumberFormat::LowerLetter,
            Self::Ordinal => ST_NumberFormat::Ordinal,
            Self::CardinalText => ST_NumberFormat::CardinalText,
            Self::OrdinalText => ST_NumberFormat::OrdinalText,
            Self::Hex => ST_NumberFormat::Hex,
            Self::Chicago => ST_NumberFormat::Chicago,
            Self::IdeographDigital => ST_NumberFormat::IdeographDigital,
            Self::JapaneseCounting => ST_NumberFormat::JapaneseCounting,
            Self::Aiueo => ST_NumberFormat::Aiueo,
            Self::Iroha => ST_NumberFormat::Iroha,
            Self::DecimalFullWidth => ST_NumberFormat::DecimalFullWidth,
            Self::DecimalHalfWidth => ST_NumberFormat::DecimalHalfWidth,
            Self::JapaneseLegal => ST_NumberFormat::JapaneseLegal,
            Self::JapaneseDigitalTenThousand => ST_NumberFormat::JapaneseDigitalTenThousand,
            Self::DecimalEnclosedCircle => ST_NumberFormat::DecimalEnclosedCircle,
            Self::DecimalFullWidth2 => ST_NumberFormat::DecimalFullWidth2,
            Self::AiueoFullWidth => ST_NumberFormat::AiueoFullWidth,
            Self::IrohaFullWidth => ST_NumberFormat::IrohaFullWidth,
            Self::DecimalZero => ST_NumberFormat::DecimalZero,
            Self::Bullet => ST_NumberFormat::Bullet,
            Self::Ganada => ST_NumberFormat::Ganada,
            Self::Chosung => ST_NumberFormat::Chosung,
            Self::DecimalEnclosedFullstop => ST_NumberFormat::DecimalEnclosedFullstop,
            Self::DecimalEnclosedParen => ST_NumberFormat::DecimalEnclosedParen,
            Self::DecimalEnclosedCircleChinese => ST_NumberFormat::DecimalEnclosedCircleChinese,
            Self::IdeographEnclosedCircle => ST_NumberFormat::IdeographEnclosedCircle,
            Self::IdeographTraditional => ST_NumberFormat::IdeographTraditional,
            Self::IdeographZodiac => ST_NumberFormat::IdeographZodiac,
            Self::IdeographZodiacTraditional => ST_NumberFormat::IdeographZodiacTraditional,
            Self::TaiwaneseCounting => ST_NumberFormat::TaiwaneseCounting,
            Self::IdeographLegalTraditional => ST_NumberFormat::IdeographLegalTraditional,
            Self::TaiwaneseCountingThousand => ST_NumberFormat::TaiwaneseCountingThousand,
            Self::TaiwaneseDigital => ST_NumberFormat::TaiwaneseDigital,
            Self::ChineseCounting => ST_NumberFormat::ChineseCounting,
            Self::ChineseLegalSimplified => ST_NumberFormat::ChineseLegalSimplified,
            Self::ChineseCountingThousand => ST_NumberFormat::ChineseCountingThousand,
            Self::KoreanDigital => ST_NumberFormat::KoreanDigital,
            Self::KoreanCounting => ST_NumberFormat::KoreanCounting,
            Self::KoreanLegal => ST_NumberFormat::KoreanLegal,
            Self::KoreanDigital2 => ST_NumberFormat::KoreanDigital2,
            Self::Hebrew1 => ST_NumberFormat::Hebrew1,
            Self::ArabicAlpha => ST_NumberFormat::ArabicAlpha,
            Self::Hebrew2 => ST_NumberFormat::Hebrew2,
            Self::ArabicAbjad => ST_NumberFormat::ArabicAbjad,
            Self::HindiVowels => ST_NumberFormat::HindiVowels,
            Self::HindiConsonants => ST_NumberFormat::HindiConsonants,
            Self::HindiNumbers => ST_NumberFormat::HindiNumbers,
            Self::HindiCounting => ST_NumberFormat::HindiCounting,
            Self::ThaiLetters => ST_NumberFormat::ThaiLetters,
            Self::ThaiNumbers => ST_NumberFormat::ThaiNumbers,
            Self::ThaiCounting => ST_NumberFormat::ThaiCounting,
            Self::VietnameseCounting => ST_NumberFormat::VietnameseCounting,
            Self::NumberInDash => ST_NumberFormat::NumberInDash,
            Self::RussianLower => ST_NumberFormat::RussianLower,
            Self::RussianUpper => ST_NumberFormat::RussianUpper,
            Self::None => ST_NumberFormat::None,
            Self::Other(value) => ST_NumberFormat::Other(value.clone()),
        }
    }
}

/// The numbering format reported by the read-only numbering projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberingFormat<'a> {
    Decimal,
    UpperRoman,
    LowerRoman,
    UpperLetter,
    LowerLetter,
    Ordinal,
    CardinalText,
    OrdinalText,
    Hex,
    Chicago,
    IdeographDigital,
    JapaneseCounting,
    Aiueo,
    Iroha,
    DecimalFullWidth,
    DecimalHalfWidth,
    JapaneseLegal,
    JapaneseDigitalTenThousand,
    DecimalEnclosedCircle,
    DecimalFullWidth2,
    AiueoFullWidth,
    IrohaFullWidth,
    DecimalZero,
    Bullet,
    Ganada,
    Chosung,
    DecimalEnclosedFullstop,
    DecimalEnclosedParen,
    DecimalEnclosedCircleChinese,
    IdeographEnclosedCircle,
    IdeographTraditional,
    IdeographZodiac,
    IdeographZodiacTraditional,
    TaiwaneseCounting,
    IdeographLegalTraditional,
    TaiwaneseCountingThousand,
    TaiwaneseDigital,
    ChineseCounting,
    ChineseLegalSimplified,
    ChineseCountingThousand,
    KoreanDigital,
    KoreanCounting,
    KoreanLegal,
    KoreanDigital2,
    Hebrew1,
    ArabicAlpha,
    Hebrew2,
    ArabicAbjad,
    HindiVowels,
    HindiConsonants,
    HindiNumbers,
    HindiCounting,
    ThaiLetters,
    ThaiNumbers,
    ThaiCounting,
    VietnameseCounting,
    NumberInDash,
    RussianLower,
    RussianUpper,
    None,
    /// A producer-defined format value retained by oxoffice.
    Other(&'a str),
}

impl<'a> NumberingFormat<'a> {
    pub(super) fn from_st(value: &'a ST_NumberFormat) -> Self {
        match value {
            ST_NumberFormat::Decimal => Self::Decimal,
            ST_NumberFormat::UpperRoman => Self::UpperRoman,
            ST_NumberFormat::LowerRoman => Self::LowerRoman,
            ST_NumberFormat::UpperLetter => Self::UpperLetter,
            ST_NumberFormat::LowerLetter => Self::LowerLetter,
            ST_NumberFormat::Ordinal => Self::Ordinal,
            ST_NumberFormat::CardinalText => Self::CardinalText,
            ST_NumberFormat::OrdinalText => Self::OrdinalText,
            ST_NumberFormat::Hex => Self::Hex,
            ST_NumberFormat::Chicago => Self::Chicago,
            ST_NumberFormat::IdeographDigital => Self::IdeographDigital,
            ST_NumberFormat::JapaneseCounting => Self::JapaneseCounting,
            ST_NumberFormat::Aiueo => Self::Aiueo,
            ST_NumberFormat::Iroha => Self::Iroha,
            ST_NumberFormat::DecimalFullWidth => Self::DecimalFullWidth,
            ST_NumberFormat::DecimalHalfWidth => Self::DecimalHalfWidth,
            ST_NumberFormat::JapaneseLegal => Self::JapaneseLegal,
            ST_NumberFormat::JapaneseDigitalTenThousand => Self::JapaneseDigitalTenThousand,
            ST_NumberFormat::DecimalEnclosedCircle => Self::DecimalEnclosedCircle,
            ST_NumberFormat::DecimalFullWidth2 => Self::DecimalFullWidth2,
            ST_NumberFormat::AiueoFullWidth => Self::AiueoFullWidth,
            ST_NumberFormat::IrohaFullWidth => Self::IrohaFullWidth,
            ST_NumberFormat::DecimalZero => Self::DecimalZero,
            ST_NumberFormat::Bullet => Self::Bullet,
            ST_NumberFormat::Ganada => Self::Ganada,
            ST_NumberFormat::Chosung => Self::Chosung,
            ST_NumberFormat::DecimalEnclosedFullstop => Self::DecimalEnclosedFullstop,
            ST_NumberFormat::DecimalEnclosedParen => Self::DecimalEnclosedParen,
            ST_NumberFormat::DecimalEnclosedCircleChinese => Self::DecimalEnclosedCircleChinese,
            ST_NumberFormat::IdeographEnclosedCircle => Self::IdeographEnclosedCircle,
            ST_NumberFormat::IdeographTraditional => Self::IdeographTraditional,
            ST_NumberFormat::IdeographZodiac => Self::IdeographZodiac,
            ST_NumberFormat::IdeographZodiacTraditional => Self::IdeographZodiacTraditional,
            ST_NumberFormat::TaiwaneseCounting => Self::TaiwaneseCounting,
            ST_NumberFormat::IdeographLegalTraditional => Self::IdeographLegalTraditional,
            ST_NumberFormat::TaiwaneseCountingThousand => Self::TaiwaneseCountingThousand,
            ST_NumberFormat::TaiwaneseDigital => Self::TaiwaneseDigital,
            ST_NumberFormat::ChineseCounting => Self::ChineseCounting,
            ST_NumberFormat::ChineseLegalSimplified => Self::ChineseLegalSimplified,
            ST_NumberFormat::ChineseCountingThousand => Self::ChineseCountingThousand,
            ST_NumberFormat::KoreanDigital => Self::KoreanDigital,
            ST_NumberFormat::KoreanCounting => Self::KoreanCounting,
            ST_NumberFormat::KoreanLegal => Self::KoreanLegal,
            ST_NumberFormat::KoreanDigital2 => Self::KoreanDigital2,
            ST_NumberFormat::Hebrew1 => Self::Hebrew1,
            ST_NumberFormat::ArabicAlpha => Self::ArabicAlpha,
            ST_NumberFormat::Hebrew2 => Self::Hebrew2,
            ST_NumberFormat::ArabicAbjad => Self::ArabicAbjad,
            ST_NumberFormat::HindiVowels => Self::HindiVowels,
            ST_NumberFormat::HindiConsonants => Self::HindiConsonants,
            ST_NumberFormat::HindiNumbers => Self::HindiNumbers,
            ST_NumberFormat::HindiCounting => Self::HindiCounting,
            ST_NumberFormat::ThaiLetters => Self::ThaiLetters,
            ST_NumberFormat::ThaiNumbers => Self::ThaiNumbers,
            ST_NumberFormat::ThaiCounting => Self::ThaiCounting,
            ST_NumberFormat::VietnameseCounting => Self::VietnameseCounting,
            ST_NumberFormat::NumberInDash => Self::NumberInDash,
            ST_NumberFormat::RussianLower => Self::RussianLower,
            ST_NumberFormat::RussianUpper => Self::RussianUpper,
            ST_NumberFormat::None => Self::None,
            ST_NumberFormat::Other(value) => Self::Other(value),
        }
    }
}

/// The layout item emitted between a list marker and paragraph content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListLevelSuffix {
    Tab,
    Space,
    Nothing,
}

/// Explicit restart behavior for a numbering level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListLevelRestart {
    /// Continue across every more-significant level.
    Never,
    /// Restart after the given zero-based, more-significant level.
    After(u32),
}

impl ListLevelRestart {
    pub(super) fn from_st(value: u32) -> Self {
        if value == 0 {
            Self::Never
        } else {
            Self::After(value - 1)
        }
    }
}

impl ListLevelSuffix {
    pub(super) fn from_st(value: ST_LvlSuffix) -> Self {
        match value {
            ST_LvlSuffix::Tab => Self::Tab,
            ST_LvlSuffix::Space => Self::Space,
            ST_LvlSuffix::Nothing => Self::Nothing,
        }
    }
}

/// Resolved reader metadata for one numbering definition level.
#[derive(Debug, Clone, PartialEq)]
pub struct NumberingLevel<'a> {
    /// The zero-based list level.
    pub level: u32,
    /// The marker format at this level.
    pub format: NumberingFormat<'a>,
    /// The exact OOXML format name, including producer-defined values.
    pub format_name: &'a str,
    /// The first marker value, defaulting to one when Word omits it.
    pub start: u32,
    /// The item emitted between the marker and paragraph content.
    pub suffix: ListLevelSuffix,
    /// The Word level-text template or bullet glyph.
    pub level_text: Option<&'a str>,
    /// The marker alignment, when explicitly configured.
    pub alignment: Option<crate::paragraph::Alignment>,
    /// The paragraph style associated with this numbering level.
    pub paragraph_style: Option<&'a str>,
    /// Explicit restart behavior. Absence means the OOXML default.
    pub restart: Option<ListLevelRestart>,
    /// Whether inherited placeholders use decimal legal numbering.
    pub legal_numbering: Option<bool>,
    /// Left indentation in twips.
    pub indent_left: Option<oxoffice_core::Twips>,
    /// Hanging indentation in twips.
    pub indent_hanging: Option<oxoffice_core::Twips>,
    /// First-line indentation in twips.
    pub indent_first_line: Option<oxoffice_core::Twips>,
    /// Typed properties applied only to the numbering marker.
    pub marker_properties: Option<&'a CT_RPr>,
    /// Producer template code from `w:tplc`.
    pub template_code: Option<&'a str>,
    /// Whether the producer marked this level tentative.
    pub tentative: Option<bool>,
    /// Whether this level retains semantic facts this projection does not model.
    pub has_unmodeled_properties: bool,
    /// Whether the list item has nonstandard paragraph-level presentation.
    pub has_paragraph_presentation: bool,
    /// Whether the marker has run-level presentation properties.
    pub has_marker_presentation: bool,
}

pub(super) fn list_level_alignment(value: ST_Jc) -> crate::paragraph::Alignment {
    match value {
        ST_Jc::Center => crate::paragraph::Alignment::Center,
        ST_Jc::Right | ST_Jc::End => crate::paragraph::Alignment::Right,
        ST_Jc::Both | ST_Jc::Distribute => crate::paragraph::Alignment::Justify,
        _ => crate::paragraph::Alignment::Left,
    }
}

/// One level of a custom list definition for [`Document::add_list_definition`].
#[derive(Debug, Clone)]
pub struct ListLevel {
    /// Numbering format for this level.
    pub format: ListNumberFormat,
    /// Starting number (defaults to 1; ignored for bullet levels).
    pub start: Option<u32>,
    level_text: Option<String>,
    suffix: Option<ListLevelSuffix>,
    alignment: Option<crate::paragraph::Alignment>,
    indent_left: Option<oxoffice_core::Twips>,
    indent_hanging: Option<oxoffice_core::Twips>,
    indent_first_line: Option<oxoffice_core::Twips>,
    marker_properties: Option<CT_RPr>,
    legal_numbering: Option<bool>,
    restart: Option<ListLevelRestart>,
    template_code: Option<String>,
    tentative: Option<bool>,
    source_level: Option<Box<CT_Lvl>>,
}

impl PartialEq for ListLevel {
    fn eq(&self, other: &Self) -> bool {
        self.format == other.format
            && self.start == other.start
            && self.level_text == other.level_text
            && self.suffix == other.suffix
            && self.alignment == other.alignment
            && self.indent_left == other.indent_left
            && self.indent_hanging == other.indent_hanging
            && self.indent_first_line == other.indent_first_line
            && self.marker_properties == other.marker_properties
            && self.legal_numbering == other.legal_numbering
            && self.restart == other.restart
            && self.template_code == other.template_code
            && self.tentative == other.tentative
    }
}

impl ListLevel {
    /// A level with the given format, starting at 1.
    pub fn new(format: ListNumberFormat) -> Self {
        ListLevel {
            format,
            start: None,
            level_text: None,
            suffix: None,
            alignment: None,
            indent_left: None,
            indent_hanging: None,
            indent_first_line: None,
            marker_properties: None,
            legal_numbering: None,
            restart: None,
            template_code: None,
            tentative: None,
            source_level: None,
        }
    }

    /// A bullet level.
    pub fn bullet() -> Self {
        Self::new(ListNumberFormat::Bullet)
    }

    /// A decimal-numbered level.
    pub fn decimal() -> Self {
        Self::new(ListNumberFormat::Decimal)
    }

    /// Override the starting number for this level.
    pub fn start(mut self, start: u32) -> Self {
        self.start = Some(start);
        self
    }

    /// Set the level-text template or bullet glyph.
    pub fn level_text(mut self, value: impl Into<String>) -> Self {
        self.level_text = Some(value.into());
        self
    }

    /// Select the content emitted after the marker.
    pub fn suffix(mut self, value: ListLevelSuffix) -> Self {
        self.suffix = Some(value);
        self
    }

    /// Set marker alignment.
    pub fn alignment(mut self, value: crate::paragraph::Alignment) -> Self {
        self.alignment = Some(value);
        self
    }

    /// Set left, hanging, and first-line indentation in twips.
    pub fn indentation(
        mut self,
        left: Option<oxoffice_core::Twips>,
        hanging: Option<oxoffice_core::Twips>,
        first_line: Option<oxoffice_core::Twips>,
    ) -> Self {
        self.indent_left = left;
        self.indent_hanging = hanging;
        self.indent_first_line = first_line;
        self
    }

    /// Set properties applied only to the numbering marker.
    pub fn marker_properties(mut self, value: CT_RPr) -> Self {
        self.marker_properties = Some(value);
        self
    }

    /// Select legal-numbering placeholder behavior.
    pub fn legal_numbering(mut self, value: bool) -> Self {
        self.legal_numbering = Some(value);
        self
    }

    /// Set explicit restart behavior.
    pub fn restart(mut self, value: ListLevelRestart) -> Self {
        self.restart = Some(value);
        self
    }

    /// Set the eight-hex-digit producer template code.
    pub fn template_code(mut self, value: impl Into<String>) -> Self {
        self.template_code = Some(value.into());
        self
    }

    /// Set the producer tentative flag.
    pub fn tentative(mut self, value: bool) -> Self {
        self.tentative = Some(value);
        self
    }

    /// Return the configured level-text template, when one was supplied.
    pub fn level_text_value(&self) -> Option<&str> {
        self.level_text.as_deref()
    }

    /// Return the configured marker suffix, when one was supplied.
    pub fn suffix_value(&self) -> Option<ListLevelSuffix> {
        self.suffix
    }

    /// Return the configured marker alignment, when one was supplied.
    pub fn alignment_value(&self) -> Option<crate::paragraph::Alignment> {
        self.alignment
    }

    /// Return the configured left, hanging, and first-line indentation.
    pub fn indentation_value(
        &self,
    ) -> (
        Option<oxoffice_core::Twips>,
        Option<oxoffice_core::Twips>,
        Option<oxoffice_core::Twips>,
    ) {
        (
            self.indent_left,
            self.indent_hanging,
            self.indent_first_line,
        )
    }

    /// Return the configured marker run properties.
    pub fn marker_properties_value(&self) -> Option<&CT_RPr> {
        self.marker_properties.as_ref()
    }

    /// Return the configured legal-numbering flag.
    pub fn legal_numbering_value(&self) -> Option<bool> {
        self.legal_numbering
    }

    /// Return the configured restart behavior.
    pub fn restart_value(&self) -> Option<ListLevelRestart> {
        self.restart
    }

    /// Return the configured producer template code.
    pub fn template_code_value(&self) -> Option<&str> {
        self.template_code.as_deref()
    }

    /// Return the configured tentative flag.
    pub fn tentative_value(&self) -> Option<bool> {
        self.tentative
    }
}

/// Owned public projection of an abstract numbering definition.
#[derive(Debug, Clone, PartialEq)]
pub struct NumberingDefinition {
    /// The abstract numbering definition identifier.
    pub id: u32,
    /// The definition's ordered levels.
    pub levels: Vec<NumberingDefinitionLevel>,
    /// Imported style links, indexed in parallel with `levels`.
    pub paragraph_style_links: Vec<Option<String>>,
    /// Whether the definition contains imported properties outside this projection.
    pub has_unmodeled_properties: bool,
}

/// One explicitly identified level in an inspected numbering definition.
#[derive(Debug, Clone, PartialEq)]
pub struct NumberingDefinitionLevel {
    /// The zero-based `w:ilvl` identifier.
    pub level: u32,
    /// The modeled properties owned by this level.
    pub properties: ListLevel,
}

/// One typed override owned by a numbering instance.
#[derive(Debug, Clone, PartialEq)]
pub struct NumberingLevelOverride {
    /// The zero-based level receiving the override.
    pub level: u32,
    /// An optional starting-value override.
    pub start: Option<u32>,
    /// An optional replacement level definition.
    pub replacement: Option<ListLevel>,
    /// Imported style link for the replacement level.
    pub paragraph_style_link: Option<String>,
    /// Whether the override contains imported properties outside this projection.
    pub has_unmodeled_properties: bool,
}

impl NumberingLevelOverride {
    /// Create an empty override for a zero-based level.
    pub fn new(level: u32) -> Self {
        Self {
            level,
            start: None,
            replacement: None,
            paragraph_style_link: None,
            has_unmodeled_properties: false,
        }
    }

    /// Set the override's starting value.
    pub fn start(mut self, value: u32) -> Self {
        self.start = Some(value);
        self
    }

    /// Replace the level definition for this instance.
    pub fn replacement(mut self, value: ListLevel) -> Self {
        self.replacement = Some(value);
        self
    }
}

/// Owned public projection of one numbering instance.
#[derive(Debug, Clone, PartialEq)]
pub struct NumberingInstance {
    /// The numbering instance identifier used by document paragraphs.
    pub id: u32,
    /// The referenced abstract numbering definition identifier.
    pub definition_id: u32,
    /// Typed overrides applied by this instance.
    pub level_overrides: Vec<NumberingLevelOverride>,
    /// Whether the instance contains imported properties outside this projection.
    pub has_unmodeled_properties: bool,
}

fn list_alignment_to_st(value: crate::paragraph::Alignment) -> ST_Jc {
    match value {
        crate::paragraph::Alignment::Left => ST_Jc::Left,
        crate::paragraph::Alignment::Center => ST_Jc::Center,
        crate::paragraph::Alignment::Right => ST_Jc::Right,
        crate::paragraph::Alignment::Justify => ST_Jc::Both,
    }
}

fn list_level_restart_to_st(value: ListLevelRestart) -> Option<u32> {
    match value {
        ListLevelRestart::Never => Some(0),
        ListLevelRestart::After(level) => level.checked_add(1),
    }
}

pub(super) fn list_level_from_ct(level: &CT_Lvl) -> ListLevel {
    let mut value = list_level_values_from_ct(level);
    value.source_level = Some(Box::new(level.clone()));
    value
}

fn list_level_values_from_ct(level: &CT_Lvl) -> ListLevel {
    ListLevel {
        format: level
            .num_fmt
            .clone()
            .map(list_number_format_from_st)
            .unwrap_or(ListNumberFormat::Decimal),
        start: level.start,
        level_text: level.lvl_text.clone(),
        suffix: level.suffix.map(ListLevelSuffix::from_st),
        alignment: level.lvl_jc.map(list_level_alignment),
        indent_left: level.ppr.as_ref().and_then(|value| value.ind_left),
        indent_hanging: level.ppr.as_ref().and_then(|value| value.ind_hanging),
        indent_first_line: level.ppr.as_ref().and_then(|value| value.ind_first_line),
        marker_properties: level.rpr.clone(),
        legal_numbering: level.legal,
        restart: level.restart.map(ListLevelRestart::from_st),
        template_code: level.template_code.clone(),
        tentative: level.tentative,
        source_level: None,
    }
}

pub(crate) fn list_number_format_from_st(value: ST_NumberFormat) -> ListNumberFormat {
    // Both enums intentionally have the same standard token set.
    match value.to_str() {
        "decimal" => ListNumberFormat::Decimal,
        "upperRoman" => ListNumberFormat::UpperRoman,
        "lowerRoman" => ListNumberFormat::LowerRoman,
        "upperLetter" => ListNumberFormat::UpperLetter,
        "lowerLetter" => ListNumberFormat::LowerLetter,
        "ordinal" => ListNumberFormat::Ordinal,
        "cardinalText" => ListNumberFormat::CardinalText,
        "ordinalText" => ListNumberFormat::OrdinalText,
        "hex" => ListNumberFormat::Hex,
        "chicago" => ListNumberFormat::Chicago,
        "ideographDigital" => ListNumberFormat::IdeographDigital,
        "japaneseCounting" => ListNumberFormat::JapaneseCounting,
        "Aiueo" => ListNumberFormat::Aiueo,
        "Iroha" => ListNumberFormat::Iroha,
        "decimalFullWidth" => ListNumberFormat::DecimalFullWidth,
        "decimalHalfWidth" => ListNumberFormat::DecimalHalfWidth,
        "japaneseLegal" => ListNumberFormat::JapaneseLegal,
        "japaneseDigitalTenThousand" => ListNumberFormat::JapaneseDigitalTenThousand,
        "decimalEnclosedCircle" => ListNumberFormat::DecimalEnclosedCircle,
        "decimalFullWidth2" => ListNumberFormat::DecimalFullWidth2,
        "aiueoFullWidth" => ListNumberFormat::AiueoFullWidth,
        "irohaFullWidth" => ListNumberFormat::IrohaFullWidth,
        "decimalZero" => ListNumberFormat::DecimalZero,
        "bullet" => ListNumberFormat::Bullet,
        "ganada" => ListNumberFormat::Ganada,
        "chosung" => ListNumberFormat::Chosung,
        "decimalEnclosedFullstop" => ListNumberFormat::DecimalEnclosedFullstop,
        "decimalEnclosedParen" => ListNumberFormat::DecimalEnclosedParen,
        "decimalEnclosedCircleChinese" => ListNumberFormat::DecimalEnclosedCircleChinese,
        "ideographEnclosedCircle" => ListNumberFormat::IdeographEnclosedCircle,
        "ideographTraditional" => ListNumberFormat::IdeographTraditional,
        "ideographZodiac" => ListNumberFormat::IdeographZodiac,
        "ideographZodiacTraditional" => ListNumberFormat::IdeographZodiacTraditional,
        "taiwaneseCounting" => ListNumberFormat::TaiwaneseCounting,
        "ideographLegalTraditional" => ListNumberFormat::IdeographLegalTraditional,
        "taiwaneseCountingThousand" => ListNumberFormat::TaiwaneseCountingThousand,
        "taiwaneseDigital" => ListNumberFormat::TaiwaneseDigital,
        "chineseCounting" => ListNumberFormat::ChineseCounting,
        "chineseLegalSimplified" => ListNumberFormat::ChineseLegalSimplified,
        "chineseCountingThousand" => ListNumberFormat::ChineseCountingThousand,
        "koreanDigital" => ListNumberFormat::KoreanDigital,
        "koreanCounting" => ListNumberFormat::KoreanCounting,
        "koreanLegal" => ListNumberFormat::KoreanLegal,
        "koreanDigital2" => ListNumberFormat::KoreanDigital2,
        "hebrew1" => ListNumberFormat::Hebrew1,
        "arabicAlpha" => ListNumberFormat::ArabicAlpha,
        "hebrew2" => ListNumberFormat::Hebrew2,
        "arabicAbjad" => ListNumberFormat::ArabicAbjad,
        "hindiVowels" => ListNumberFormat::HindiVowels,
        "hindiConsonants" => ListNumberFormat::HindiConsonants,
        "hindiNumbers" => ListNumberFormat::HindiNumbers,
        "hindiCounting" => ListNumberFormat::HindiCounting,
        "thaiLetters" => ListNumberFormat::ThaiLetters,
        "thaiNumbers" => ListNumberFormat::ThaiNumbers,
        "thaiCounting" => ListNumberFormat::ThaiCounting,
        "vietnameseCounting" => ListNumberFormat::VietnameseCounting,
        "numberInDash" => ListNumberFormat::NumberInDash,
        "russianLower" => ListNumberFormat::RussianLower,
        "russianUpper" => ListNumberFormat::RussianUpper,
        "none" => ListNumberFormat::None,
        other => ListNumberFormat::Other(other.to_owned()),
    }
}

pub(super) fn numbering_level_has_unmodeled(level: &CT_Lvl) -> bool {
    let unprojected_paragraph_properties = level.ppr.as_ref().is_some_and(|properties| {
        let projected = CT_PPr {
            ind_left: properties.ind_left,
            ind_hanging: properties.ind_hanging,
            ind_first_line: properties.ind_first_line,
            ..CT_PPr::default()
        };
        properties != &projected
    });
    !level.extra_xml.is_empty()
        || !level.extra_attributes.is_empty()
        || level
            .start_raw
            .as_ref()
            .is_some_and(|(_, raw, prefixes)| typed_numbering_leaf_has_unmodeled(raw, prefixes))
        || level
            .num_fmt_raw
            .as_ref()
            .is_some_and(|(_, raw, prefixes)| typed_numbering_leaf_has_unmodeled(raw, prefixes))
        || level
            .p_style_raw
            .as_ref()
            .is_some_and(|(_, raw, prefixes)| typed_numbering_leaf_has_unmodeled(raw, prefixes))
        || level
            .restart_raw
            .as_ref()
            .is_some_and(|(_, raw, prefixes)| typed_numbering_leaf_has_unmodeled(raw, prefixes))
        || level
            .legal_raw
            .as_ref()
            .is_some_and(|(_, raw, prefixes)| typed_numbering_leaf_has_unmodeled(raw, prefixes))
        || level
            .suffix_raw
            .as_ref()
            .is_some_and(|(_, raw, prefixes)| typed_numbering_leaf_has_unmodeled(raw, prefixes))
        || level
            .lvl_text_raw
            .as_ref()
            .is_some_and(|(_, raw, prefixes)| typed_numbering_leaf_has_unmodeled(raw, prefixes))
        || level
            .lvl_jc_raw
            .as_ref()
            .is_some_and(|(_, raw, prefixes)| typed_numbering_leaf_has_unmodeled(raw, prefixes))
        || level.ppr_raw.is_some()
        || level.rpr_raw.is_some()
        || unprojected_paragraph_properties
        || matches!(level.num_fmt.as_ref(), Some(ST_NumberFormat::Other(_)))
}

/// Whether a restart can copy this instance into a new one: everything it
/// holds is modelled, but namespace declarations and Word's
/// `w16cid:durableId`, which names the source instance and which the copy
/// must not take. The durable id is matched with the prefix Word writes, so
/// an attribute of that name under another prefix is refused, not dropped.
pub(super) fn restart_can_copy(instance: &CT_Num) -> bool {
    instance.extra_xml.is_empty()
        && instance.extra_attributes.iter().all(|(name, _)| {
            name == "xmlns" || name.starts_with("xmlns:") || name == "w16cid:durableId"
        })
        && !instance
            .abstract_num_id_raw
            .as_ref()
            .is_some_and(|(_, raw, prefixes)| typed_numbering_leaf_has_unmodeled(raw, prefixes))
        && !instance
            .level_overrides
            .iter()
            .any(numbering_override_has_unmodeled)
}

pub(super) fn numbering_override_has_unmodeled(value: &CT_NumLvl) -> bool {
    !value.extra_xml.is_empty()
        || !value.extra_attributes.is_empty()
        || value
            .start_override_raw
            .as_ref()
            .is_some_and(|(_, raw, prefixes)| typed_numbering_leaf_has_unmodeled(raw, prefixes))
        || value
            .level
            .as_ref()
            .is_some_and(numbering_level_has_unmodeled)
}

pub(super) fn validate_list_level(level: u32, value: &ListLevel) -> Result<()> {
    if level > 8 {
        return Err(Error::Other(format!(
            "numbering level {level} is outside the supported range 0 through 8"
        )));
    }
    if value.indent_hanging.is_some() && value.indent_first_line.is_some() {
        return Err(Error::Other(format!(
            "numbering level {level} cannot have both hanging and first-line indentation"
        )));
    }
    if let Some(ListLevelRestart::After(owner)) = value.restart
        && owner >= level
    {
        return Err(Error::Other(format!(
            "numbering level {level} cannot restart after level {owner}"
        )));
    }
    if let Some(template_code) = &value.template_code
        && (template_code.len() != 8 || !template_code.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(Error::Other(format!(
            "numbering level {level} template code must contain eight hexadecimal digits"
        )));
    }
    if let Some(text) = &value.level_text {
        let bytes = text.as_bytes();
        let mut position = 0usize;
        while position < bytes.len() {
            if bytes[position] != b'%' {
                position += 1;
                continue;
            }
            let Some(next) = bytes.get(position + 1).copied() else {
                return Err(Error::Other(format!(
                    "numbering level {level} has an incomplete placeholder"
                )));
            };
            if next == b'%' {
                position += 2;
                continue;
            }
            if !(b'1'..=b'9').contains(&next) || u32::from(next - b'1') > level {
                return Err(Error::Other(format!(
                    "numbering level {level} has an invalid placeholder %{}",
                    char::from(next)
                )));
            }
            position += 2;
        }
    }
    Ok(())
}

fn default_list_level_text(level: u32, format: &ListNumberFormat) -> String {
    const BULLETS: [&str; 3] = ["\u{2022}", "\u{25E6}", "\u{25AA}"];
    match format {
        ListNumberFormat::Bullet => BULLETS[level as usize % BULLETS.len()].to_owned(),
        ListNumberFormat::None => String::new(),
        _ => format!("%{}.", level + 1),
    }
}

pub(super) fn ct_level_from_list(level: u32, value: &ListLevel) -> Result<CT_Lvl> {
    validate_list_level(level, value)?;
    let mut result = CT_Lvl::new(level);
    result.template_code.clone_from(&value.template_code);
    result.tentative = value.tentative;
    result.start = Some(value.start.unwrap_or(1));
    result.num_fmt = Some(value.format.to_st());
    result.restart = value.restart.and_then(list_level_restart_to_st);
    result.legal = value.legal_numbering;
    result.suffix = value.suffix.map(|suffix| match suffix {
        ListLevelSuffix::Tab => ST_LvlSuffix::Tab,
        ListLevelSuffix::Space => ST_LvlSuffix::Space,
        ListLevelSuffix::Nothing => ST_LvlSuffix::Nothing,
    });
    result.lvl_text = Some(
        value
            .level_text
            .clone()
            .unwrap_or_else(|| default_list_level_text(level, &value.format)),
    );
    result.lvl_jc = Some(
        value
            .alignment
            .map(list_alignment_to_st)
            .unwrap_or(ST_Jc::Left),
    );
    let default_left = (level + 1)
        .checked_mul(720)
        .and_then(|value| i32::try_from(value).ok())
        .map(oxoffice_core::Twips)
        .ok_or_else(|| Error::Other(format!("numbering level {level} indentation overflow")))?;
    result.ppr = Some(CT_PPr {
        ind_left: Some(value.indent_left.unwrap_or(default_left)),
        ind_hanging: Some(value.indent_hanging.unwrap_or(oxoffice_core::Twips(360))),
        ind_first_line: value.indent_first_line,
        ..CT_PPr::default()
    });
    if value.indent_first_line.is_some() && value.indent_hanging.is_none() {
        result
            .ppr
            .as_mut()
            .expect("created paragraph properties")
            .ind_hanging = None;
    }
    result.rpr.clone_from(&value.marker_properties);
    Ok(result)
}

pub(super) fn merge_list_level(existing: &CT_Lvl, value: &ListLevel) -> Result<CT_Lvl> {
    if value.source_level.as_deref() == Some(existing) {
        let source = list_level_values_from_ct(existing);
        let mut merged = existing.clone();
        if value.format != source.format {
            merged.num_fmt = Some(value.format.to_st());
        }
        if value.start != source.start {
            merged.start = value.start;
        }
        if value.level_text != source.level_text {
            merged.lvl_text.clone_from(&value.level_text);
        }
        if value.suffix != source.suffix {
            merged.suffix = value.suffix.map(|suffix| match suffix {
                ListLevelSuffix::Tab => ST_LvlSuffix::Tab,
                ListLevelSuffix::Space => ST_LvlSuffix::Space,
                ListLevelSuffix::Nothing => ST_LvlSuffix::Nothing,
            });
        }
        if value.alignment != source.alignment {
            merged.lvl_jc = value.alignment.map(list_alignment_to_st);
        }
        if value.restart != source.restart {
            merged.restart = value.restart.and_then(list_level_restart_to_st);
        }
        if value.legal_numbering != source.legal_numbering {
            merged.legal = value.legal_numbering;
        }
        if value.template_code != source.template_code {
            merged.template_code.clone_from(&value.template_code);
        }
        if value.tentative != source.tentative {
            merged.tentative = value.tentative;
        }
        if value.indentation_value() != source.indentation_value() {
            let properties = merged.ppr.get_or_insert_with(CT_PPr::default);
            properties.ind_left = value.indent_left;
            properties.ind_hanging = value.indent_hanging;
            properties.ind_first_line = value.indent_first_line;
        }
        if value.marker_properties != source.marker_properties {
            merged.rpr.clone_from(&value.marker_properties);
        }
        return Ok(merged);
    }
    let mut authored = ct_level_from_list(existing.ilvl, value)?;
    authored.start_raw.clone_from(&existing.start_raw);
    authored.num_fmt_raw.clone_from(&existing.num_fmt_raw);
    authored.p_style.clone_from(&existing.p_style);
    authored.p_style_raw.clone_from(&existing.p_style_raw);
    authored.restart_raw.clone_from(&existing.restart_raw);
    authored.legal_raw.clone_from(&existing.legal_raw);
    authored.suffix_raw.clone_from(&existing.suffix_raw);
    authored.lvl_text_raw.clone_from(&existing.lvl_text_raw);
    authored.lvl_jc_raw.clone_from(&existing.lvl_jc_raw);
    authored.extra_xml.clone_from(&existing.extra_xml);
    authored
        .extra_attributes
        .clone_from(&existing.extra_attributes);
    authored.ppr_raw.clone_from(&existing.ppr_raw);
    authored.rpr_raw.clone_from(&existing.rpr_raw);
    Ok(authored)
}

fn reject_conflicting_raw_level_attribute(level: &CT_Lvl, local_name: &[u8]) -> Result<()> {
    if level.extra_attributes.iter().any(|(name, _)| {
        name.as_bytes()
            .rsplit(|byte| *byte == b':')
            .next()
            .is_some_and(|local| local == local_name)
    }) {
        return Err(Error::Other(format!(
            "cannot replace unmodeled numbering level attribute '{}'",
            String::from_utf8_lossy(local_name)
        )));
    }
    Ok(())
}

pub(super) fn validate_numbering_level_slice(levels: &[ListLevel]) -> Result<()> {
    if levels.is_empty() || levels.len() > 9 {
        return Err(Error::Other(format!(
            "numbering definitions require 1 through 9 levels, received {}",
            levels.len()
        )));
    }
    for (level, value) in levels.iter().enumerate() {
        validate_list_level(level as u32, value)?;
    }
    Ok(())
}

pub(super) fn validate_numbering_definition_levels(
    levels: &[NumberingDefinitionLevel],
) -> Result<()> {
    if levels.is_empty() || levels.len() > 9 {
        return Err(Error::Other(format!(
            "numbering definitions require 1 through 9 levels, received {}",
            levels.len()
        )));
    }
    let mut seen = HashSet::new();
    for value in levels {
        if !seen.insert(value.level) {
            return Err(Error::Other(format!(
                "numbering definition contains duplicate level {}",
                value.level
            )));
        }
        validate_list_level(value.level, &value.properties)?;
    }
    Ok(())
}

pub(super) fn validate_ct_numbering_level(level: &CT_Lvl) -> Result<()> {
    if level.template_code.is_some() {
        reject_conflicting_raw_level_attribute(level, b"tplc")?;
    }
    if level.tentative.is_some() {
        reject_conflicting_raw_level_attribute(level, b"tentative")?;
    }
    validate_list_level(level.ilvl, &list_level_from_ct(level))
}

pub(super) fn numbering_override_from_public(value: &NumberingLevelOverride) -> Result<CT_NumLvl> {
    if value.level > 8 {
        return Err(Error::Other(format!(
            "numbering override level {} is outside the supported range 0 through 8",
            value.level
        )));
    }
    if value.paragraph_style_link.is_some() {
        return Err(Error::Other(
            "standalone numbering style-link mutation is not supported yet".to_owned(),
        ));
    }
    if value.has_unmodeled_properties {
        return Err(Error::Other(
            "a new numbering override cannot claim imported unmodeled properties".to_owned(),
        ));
    }
    let mut result = CT_NumLvl::new(value.level);
    result.start_override = value.start;
    result.level = value
        .replacement
        .as_ref()
        .map(|level| ct_level_from_list(value.level, level))
        .transpose()?;
    Ok(result)
}

pub(super) fn numbering_override_for_update(
    existing: Option<&CT_NumLvl>,
    value: &NumberingLevelOverride,
) -> Result<CT_NumLvl> {
    let Some(existing) = existing else {
        return numbering_override_from_public(value);
    };
    let existing_style = existing
        .level
        .as_ref()
        .and_then(|level| level.p_style.as_deref());
    if value.paragraph_style_link.as_deref() != existing_style {
        return Err(Error::Other(
            "standalone numbering style-link mutation is not supported yet".to_owned(),
        ));
    }
    let mut result = existing.clone();
    result.start_override = value.start;
    result.level = match (&existing.level, &value.replacement) {
        (Some(existing), Some(replacement)) => Some(merge_list_level(existing, replacement)?),
        (None, Some(replacement)) => Some(ct_level_from_list(value.level, replacement)?),
        (_, None) => None,
    };
    Ok(result)
}

pub(super) fn xml_paragraph_numbering_properties(xml: &[u8]) -> Result<Vec<CT_PPr>> {
    struct ParagraphState {
        depth: usize,
        ppr_depth: Option<usize>,
        numpr_depth: Option<usize>,
        properties: CT_PPr,
    }

    fn word_value(reader: &NsReader<&[u8]>, element: &BytesStart<'_>) -> Result<Option<String>> {
        for attribute in element.attributes() {
            let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
            let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
            if matches!(
                namespace,
                ResolveResult::Bound(value) if value.as_ref() == WORD_NAMESPACE.as_bytes()
            ) && local.as_ref() == b"val"
            {
                return attribute
                    .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                    .map(|value| Some(value.into_owned()))
                    .map_err(|error| Error::Other(error.to_string()));
            }
        }
        Ok(None)
    }

    fn apply_leaf(
        reader: &NsReader<&[u8]>,
        state: &mut ParagraphState,
        depth: usize,
        local_name: &[u8],
        element: &BytesStart<'_>,
    ) -> Result<()> {
        if local_name == b"pStyle" && state.ppr_depth == depth.checked_sub(1) {
            state.properties.style_id = word_value(reader, element)?;
        } else if state.numpr_depth == depth.checked_sub(1) {
            if local_name == b"numId" {
                state.properties.num_id =
                    word_value(reader, element)?.and_then(|value| value.parse::<u32>().ok());
            } else if local_name == b"ilvl" {
                state.properties.num_ilvl =
                    word_value(reader, element)?.and_then(|value| value.parse::<u32>().ok());
            }
        }
        Ok(())
    }

    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    let mut depth = 0_usize;
    let mut paragraphs = Vec::new();
    let mut result = Vec::new();
    // Open `mc:AlternateContent` elements: their depth and whether a branch
    // of each was read.
    let mut alternates: Vec<(usize, bool)> = Vec::new();
    loop {
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(error.to_string()))?;
        match event {
            Event::Start(ref element) => {
                let is_word = matches!(
                    namespace,
                    ResolveResult::Bound(value)
                        if value.as_ref() == WORD_NAMESPACE.as_bytes()
                );
                // Word writes each text box twice, in `mc:Choice` and in
                // `mc:Fallback`: its paragraphs are read once, from the
                // first branch of each `mc:AlternateContent`.
                let markup = matches!(
                    namespace,
                    ResolveResult::Bound(value)
                        if value.as_ref() == oxoffice_docx_oxml::namespace::MC_NS.as_bytes()
                );
                if markup && element.local_name().as_ref() == b"AlternateContent" {
                    alternates.push((depth, false));
                }
                let branch =
                    markup && matches!(element.local_name().as_ref(), b"Choice" | b"Fallback");
                let read_before = branch
                    && alternates
                        .last()
                        .is_some_and(|(at, read)| *at + 1 == depth && *read);
                if branch
                    && let Some((at, read)) = alternates.last_mut()
                    && *at + 1 == depth
                {
                    *read = true;
                }
                if read_before {
                    let end = element.to_end().into_owned();
                    reader
                        .read_to_end_into(end.name(), &mut Vec::new())
                        .map_err(|error| Error::Other(error.to_string()))?;
                    buffer.clear();
                    continue;
                }
                let local_name = element.local_name();
                if is_word && local_name.as_ref() == b"p" {
                    paragraphs.push(ParagraphState {
                        depth,
                        ppr_depth: None,
                        numpr_depth: None,
                        properties: CT_PPr::default(),
                    });
                } else if is_word && let Some(state) = paragraphs.last_mut() {
                    if local_name.as_ref() == b"pPr" && depth == state.depth + 1 {
                        state.ppr_depth = Some(depth);
                    } else if local_name.as_ref() == b"numPr"
                        && state.ppr_depth == depth.checked_sub(1)
                    {
                        state.numpr_depth = Some(depth);
                    } else {
                        apply_leaf(&reader, state, depth, local_name.as_ref(), element)?;
                    }
                }
                depth += 1;
            }
            Event::Empty(ref element) => {
                let is_word = matches!(
                    namespace,
                    ResolveResult::Bound(value)
                        if value.as_ref() == WORD_NAMESPACE.as_bytes()
                );
                let local_name = element.local_name();
                if is_word && local_name.as_ref() == b"p" {
                    result.push(CT_PPr::default());
                } else if is_word && let Some(state) = paragraphs.last_mut() {
                    apply_leaf(&reader, state, depth, local_name.as_ref(), element)?;
                }
            }
            Event::End(ref element) => {
                depth = depth.saturating_sub(1);
                if alternates.last().is_some_and(|(at, _)| *at == depth) {
                    alternates.pop();
                }
                let is_word = matches!(
                    namespace,
                    ResolveResult::Bound(value)
                        if value.as_ref() == WORD_NAMESPACE.as_bytes()
                );
                if is_word && element.local_name().as_ref() == b"p" {
                    if let Some(paragraph) = paragraphs.pop_if(|state| state.depth == depth) {
                        result.push(paragraph.properties);
                    }
                } else if let Some(state) = paragraphs.last_mut() {
                    if state.numpr_depth == Some(depth) {
                        state.numpr_depth = None;
                    }
                    if state.ppr_depth == Some(depth) {
                        state.ppr_depth = None;
                        state.numpr_depth = None;
                    }
                }
            }
            Event::Eof => return Ok(result),
            _ => {}
        }
        buffer.clear();
    }
}

pub(super) fn xml_references_numbering_instance(xml: &[u8], id: u32) -> Result<bool> {
    let mut reader = NsReader::from_reader(xml);
    let mut buffer = Vec::new();
    loop {
        let (namespace, event) = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| Error::Other(error.to_string()))?;
        match event {
            Event::Start(ref element) | Event::Empty(ref element)
                if matches!(
                    namespace,
                    ResolveResult::Bound(value)
                        if value.as_ref() == WORD_NAMESPACE.as_bytes()
                ) && element.local_name().as_ref() == b"numId" =>
            {
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|error| Error::Other(error.to_string()))?;
                    let (namespace, local) = reader.resolver().resolve_attribute(attribute.key);
                    if !matches!(
                        namespace,
                        ResolveResult::Bound(value)
                            if value.as_ref() == WORD_NAMESPACE.as_bytes()
                    ) || local.as_ref() != b"val"
                    {
                        continue;
                    }
                    let value = attribute
                        .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                        .map_err(|error| Error::Other(error.to_string()))?;
                    if value.parse::<u32>().ok() == Some(id) {
                        return Ok(true);
                    }
                }
            }
            Event::Eof => return Ok(false),
            _ => {}
        }
        buffer.clear();
    }
}

/// The numbering and styles of a glossary document (`word/glossary/`),
/// which resolves its paragraphs' numbering against its own parts, not the
/// main document's.
pub(super) struct GlossaryNumbering {
    pub(super) part_names: Vec<String>,
    pub(super) numbering: Option<CT_Numbering>,
    pub(super) styles: CT_Styles,
    /// Glossary documents whose numbering or styles could not be read.
    pub(super) skipped: Vec<String>,
}

impl Document {
    /// The glossary documents of the package with their own numbering and
    /// styles, read from the package as it is.
    pub(super) fn glossary_numbering(&self) -> Result<GlossaryNumbering> {
        let mut glossary = GlossaryNumbering {
            part_names: Vec::new(),
            numbering: None,
            styles: CT_Styles::default(),
            skipped: Vec::new(),
        };
        let Some(relationships) = self.package.get_part_rels(&self.doc_part_name) else {
            return Ok(glossary);
        };
        for relationship in relationships.items.iter().filter(|relationship| {
            relationship_is_internal(relationship)
                && relationship.rel_type == rel_types::GLOSSARY_DOCUMENT
        }) {
            let part_name =
                OpcPackage::resolve_rel_target(&self.doc_part_name, &relationship.target);
            if let Some(owned) = self.package.get_part_rels(&part_name) {
                for owned in owned
                    .items
                    .iter()
                    .filter(|owned| relationship_is_internal(owned))
                {
                    let target = OpcPackage::resolve_rel_target(&part_name, &owned.target);
                    let Some(bytes) = self.package.get_part(&target) else {
                        continue;
                    };
                    // A glossary part that cannot be read leaves the
                    // glossary out of the checks, as before it had scopes.
                    if owned.rel_type == rel_types::NUMBERING {
                        let Ok(numbering) = CT_Numbering::from_xml(bytes) else {
                            glossary.skipped.push(part_name.clone());
                            continue;
                        };
                        glossary.numbering = Some(numbering);
                    } else if owned.rel_type == rel_types::STYLES {
                        let Ok(styles) = CT_Styles::from_xml(bytes) else {
                            glossary.skipped.push(part_name.clone());
                            continue;
                        };
                        glossary.styles = styles;
                    }
                }
            }
            glossary.part_names.push(part_name);
        }
        Ok(glossary)
    }
}

/// The numbering instance and level a paragraph with direct properties
/// `direct` is numbered at, resolved against `numbering` and `styles`:
/// direct numbering, then the paragraph style's, the level of a style a
/// definition links last. `None` when it is not numbered.
pub(super) fn effective_numbering_reference(
    numbering: Option<&CT_Numbering>,
    styles: &CT_Styles,
    direct: &CT_PPr,
) -> Option<(u32, u32)> {
    let style_id = direct.style_id.as_deref().or_else(|| {
        styles
            .get_default(StyleType::Paragraph)
            .map(|style| style.style_id.as_str())
    });
    let inherited = style::resolve_paragraph_properties(style_id, styles);
    let num_id = direct.num_id.or(inherited.num_id).filter(|id| *id != 0)?;
    let level = direct
        .num_ilvl
        .or(inherited.num_ilvl)
        .or_else(|| numbering_level_for_style(numbering, styles, num_id, style_id))
        .unwrap_or(0);
    Some((num_id, level))
}

/// The level of definition `num_id` that names `style_id` or a style it is
/// based on as its paragraph style.
pub(super) fn numbering_level_for_style(
    numbering: Option<&CT_Numbering>,
    styles: &CT_Styles,
    num_id: u32,
    style_id: Option<&str>,
) -> Option<u32> {
    let definition = numbering?.get_abstract_num_for(num_id)?;
    let mut style_id = style_id?;
    for _ in 0..styles.styles.len() {
        if let Some(level) = definition
            .levels
            .iter()
            .find(|level| level.p_style.as_deref() == Some(style_id))
        {
            return Some(level.ilvl);
        }
        style_id = styles.get_by_id(style_id)?.based_on.as_deref()?;
    }
    None
}

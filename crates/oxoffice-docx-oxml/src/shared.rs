//! Shared simple types and enums used across OOXML elements.

use crate::error::{OxmlError, Result};

/// `ST_Jc` — Paragraph justification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ST_Jc {
    Start,
    End,
    Center,
    Both,
    Distribute,
    Left,
    Right,
}

impl ST_Jc {
    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "start" => Ok(ST_Jc::Start),
            "end" => Ok(ST_Jc::End),
            "left" => Ok(ST_Jc::Left),
            "right" => Ok(ST_Jc::Right),
            "center" => Ok(ST_Jc::Center),
            // Kashida justification stretches Arabic text by elongating the
            // connecting stroke rather than by widening spaces. Shaping that
            // faithfully is beyond this crate, and justified is what the three
            // values mean at the paragraph level. Rejecting them instead failed
            // the whole document open.
            "both" | "justify" | "lowKashida" | "mediumKashida" | "highKashida" => Ok(ST_Jc::Both),
            "distribute" => Ok(ST_Jc::Distribute),
            _ => Err(OxmlError::InvalidValue(format!("invalid ST_Jc: {s}"))),
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            ST_Jc::Start => "start",
            ST_Jc::End => "end",
            ST_Jc::Left => "left",
            ST_Jc::Right => "right",
            ST_Jc::Center => "center",
            ST_Jc::Both => "both",
            ST_Jc::Distribute => "distribute",
        }
    }
}

/// `ST_OnOff` — Boolean toggle, can be represented as "true"/"false", "1"/"0", or attribute absence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ST_OnOff {
    On,
    Off,
}

impl ST_OnOff {
    /// Read one of the six spellings of the transitional schema (`true`,
    /// `1`, `on`, `false`, `0`, `off`); `None` for any other value.
    ///
    /// Every on/off attribute of the Word model is read through this, so
    /// that no reader takes `on` as off.
    pub fn parse(value: &str) -> Option<bool> {
        match value {
            "true" | "1" | "on" => Some(true),
            "false" | "0" | "off" => Some(false),
            _ => None,
        }
    }

    pub fn from_str_or_default(s: Option<&str>) -> Self {
        match s {
            // If the attribute is absent or empty, the element presence means "on"
            None | Some("") => ST_OnOff::On,
            Some(value) if Self::parse(value) == Some(true) => ST_OnOff::On,
            Some(_) => ST_OnOff::Off,
        }
    }

    pub fn is_on(self) -> bool {
        self == ST_OnOff::On
    }

    pub fn to_str(self) -> &'static str {
        match self {
            ST_OnOff::On => "true",
            ST_OnOff::Off => "false",
        }
    }
}

/// `ST_UnderlineType` — Underline styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ST_Underline {
    None,
    Single,
    Words,
    Double,
    Thick,
    Dotted,
    Dash,
    DotDash,
    DotDotDash,
    Wave,
    DottedHeavy,
    DashedHeavy,
    DashLong,
    DashLongHeavy,
    DashDotHeavy,
    DashDotDotHeavy,
    WavyHeavy,
    WavyDouble,
}

impl ST_Underline {
    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "none" => Ok(ST_Underline::None),
            "single" => Ok(ST_Underline::Single),
            "words" => Ok(ST_Underline::Words),
            "double" => Ok(ST_Underline::Double),
            "thick" => Ok(ST_Underline::Thick),
            "dotted" => Ok(ST_Underline::Dotted),
            "dash" => Ok(ST_Underline::Dash),
            "dotDash" => Ok(ST_Underline::DotDash),
            "dotDotDash" => Ok(ST_Underline::DotDotDash),
            "wave" => Ok(ST_Underline::Wave),
            "dottedHeavy" => Ok(ST_Underline::DottedHeavy),
            "dashedHeavy" => Ok(ST_Underline::DashedHeavy),
            "dashLong" => Ok(ST_Underline::DashLong),
            "dashLongHeavy" => Ok(ST_Underline::DashLongHeavy),
            "dashDotHeavy" => Ok(ST_Underline::DashDotHeavy),
            "dashDotDotHeavy" => Ok(ST_Underline::DashDotDotHeavy),
            "wavyHeavy" => Ok(ST_Underline::WavyHeavy),
            "wavyDouble" => Ok(ST_Underline::WavyDouble),
            _ => Err(OxmlError::InvalidValue(format!(
                "invalid ST_Underline: {s}"
            ))),
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            ST_Underline::None => "none",
            ST_Underline::Single => "single",
            ST_Underline::Words => "words",
            ST_Underline::Double => "double",
            ST_Underline::Thick => "thick",
            ST_Underline::Dotted => "dotted",
            ST_Underline::Dash => "dash",
            ST_Underline::DotDash => "dotDash",
            ST_Underline::DotDotDash => "dotDotDash",
            ST_Underline::Wave => "wave",
            ST_Underline::DottedHeavy => "dottedHeavy",
            ST_Underline::DashedHeavy => "dashedHeavy",
            ST_Underline::DashLong => "dashLong",
            ST_Underline::DashLongHeavy => "dashLongHeavy",
            ST_Underline::DashDotHeavy => "dashDotHeavy",
            ST_Underline::DashDotDotHeavy => "dashDotDotHeavy",
            ST_Underline::WavyHeavy => "wavyHeavy",
            ST_Underline::WavyDouble => "wavyDouble",
        }
    }
}

/// `ST_Border` — Border styles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ST_Border {
    None,
    Nil,
    Single,
    Thick,
    Double,
    Dotted,
    Dashed,
    DotDash,
    DotDotDash,
    Triple,
    ThinThickSmallGap,
    ThickThinSmallGap,
    ThinThickThinSmallGap,
    ThinThickMediumGap,
    ThickThinMediumGap,
    ThinThickThinMediumGap,
    ThinThickLargeGap,
    ThickThinLargeGap,
    ThinThickThinLargeGap,
    Wave,
    DoubleWave,
    DashSmallGap,
    DashDotStroked,
    ThreeDEmboss,
    ThreeDEngrave,
    Outset,
    Inset,
    /// One of the picture borders of the enumeration, such as `apples`.
    Art(BorderArt),
}

impl ST_Border {
    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "none" => Ok(Self::None),
            "nil" => Ok(Self::Nil),
            "single" => Ok(Self::Single),
            "thick" => Ok(Self::Thick),
            "double" => Ok(Self::Double),
            "dotted" => Ok(Self::Dotted),
            "dashed" => Ok(Self::Dashed),
            "dotDash" => Ok(Self::DotDash),
            "dotDotDash" => Ok(Self::DotDotDash),
            "triple" => Ok(Self::Triple),
            "thinThickSmallGap" => Ok(Self::ThinThickSmallGap),
            "thickThinSmallGap" => Ok(Self::ThickThinSmallGap),
            "thinThickThinSmallGap" => Ok(Self::ThinThickThinSmallGap),
            "thinThickMediumGap" => Ok(Self::ThinThickMediumGap),
            "thickThinMediumGap" => Ok(Self::ThickThinMediumGap),
            "thinThickThinMediumGap" => Ok(Self::ThinThickThinMediumGap),
            "thinThickLargeGap" => Ok(Self::ThinThickLargeGap),
            "thickThinLargeGap" => Ok(Self::ThickThinLargeGap),
            "thinThickThinLargeGap" => Ok(Self::ThinThickThinLargeGap),
            "wave" => Ok(Self::Wave),
            "doubleWave" => Ok(Self::DoubleWave),
            "dashSmallGap" => Ok(Self::DashSmallGap),
            "dashDotStroked" => Ok(Self::DashDotStroked),
            "threeDEmboss" => Ok(Self::ThreeDEmboss),
            "threeDEngrave" => Ok(Self::ThreeDEngrave),
            "outset" => Ok(Self::Outset),
            "inset" => Ok(Self::Inset),
            _ => BorderArt::from_str(s)
                .map(Self::Art)
                .ok_or_else(|| OxmlError::InvalidValue(format!("invalid ST_Border: {s}"))),
        }
    }

    /// Whether this token suppresses border painting.
    pub fn is_none(self) -> bool {
        matches!(self, Self::None | Self::Nil)
    }

    pub fn to_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Nil => "nil",
            Self::Single => "single",
            Self::Thick => "thick",
            Self::Double => "double",
            Self::Dotted => "dotted",
            Self::Dashed => "dashed",
            Self::DotDash => "dotDash",
            Self::DotDotDash => "dotDotDash",
            Self::Triple => "triple",
            Self::ThinThickSmallGap => "thinThickSmallGap",
            Self::ThickThinSmallGap => "thickThinSmallGap",
            Self::ThinThickThinSmallGap => "thinThickThinSmallGap",
            Self::ThinThickMediumGap => "thinThickMediumGap",
            Self::ThickThinMediumGap => "thickThinMediumGap",
            Self::ThinThickThinMediumGap => "thinThickThinMediumGap",
            Self::ThinThickLargeGap => "thinThickLargeGap",
            Self::ThickThinLargeGap => "thickThinLargeGap",
            Self::ThinThickThinLargeGap => "thinThickThinLargeGap",
            Self::Wave => "wave",
            Self::DoubleWave => "doubleWave",
            Self::DashSmallGap => "dashSmallGap",
            Self::DashDotStroked => "dashDotStroked",
            Self::ThreeDEmboss => "threeDEmboss",
            Self::ThreeDEngrave => "threeDEngrave",
            Self::Outset => "outset",
            Self::Inset => "inset",
            Self::Art(art) => art.as_str(),
        }
    }
}

/// A picture border of `ST_Border`, as its index in the schema order.
///
/// Only [`ST_Border::from_str`] makes one, so it always names a value of the
/// enumeration and serializes back to that same token. A one-byte index
/// keeps `ST_Border`, and every property set holding one, as small as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BorderArt(u8);

impl BorderArt {
    fn from_str(s: &str) -> Option<Self> {
        BORDER_ART
            .iter()
            .position(|art| *art == s)
            .and_then(|index| u8::try_from(index).ok())
            .map(Self)
    }

    /// The schema token, such as `apples`.
    pub fn as_str(self) -> &'static str {
        BORDER_ART[usize::from(self.0)]
    }
}

/// The picture borders of `ST_Border`, in schema order.
const BORDER_ART: [&str; 166] = [
    "apples",
    "archedScallops",
    "babyPacifier",
    "babyRattle",
    "balloons3Colors",
    "balloonsHotAir",
    "basicBlackDashes",
    "basicBlackDots",
    "basicBlackSquares",
    "basicThinLines",
    "basicWhiteDashes",
    "basicWhiteDots",
    "basicWhiteSquares",
    "basicWideInline",
    "basicWideMidline",
    "basicWideOutline",
    "bats",
    "birds",
    "birdsFlight",
    "cabins",
    "cakeSlice",
    "candyCorn",
    "celticKnotwork",
    "certificateBanner",
    "chainLink",
    "champagneBottle",
    "checkedBarBlack",
    "checkedBarColor",
    "checkered",
    "christmasTree",
    "circlesLines",
    "circlesRectangles",
    "classicalWave",
    "clocks",
    "compass",
    "confetti",
    "confettiGrays",
    "confettiOutline",
    "confettiStreamers",
    "confettiWhite",
    "cornerTriangles",
    "couponCutoutDashes",
    "couponCutoutDots",
    "crazyMaze",
    "creaturesButterfly",
    "creaturesFish",
    "creaturesInsects",
    "creaturesLadyBug",
    "crossStitch",
    "cup",
    "decoArch",
    "decoArchColor",
    "decoBlocks",
    "diamondsGray",
    "doubleD",
    "doubleDiamonds",
    "earth1",
    "earth2",
    "earth3",
    "eclipsingSquares1",
    "eclipsingSquares2",
    "eggsBlack",
    "fans",
    "film",
    "firecrackers",
    "flowersBlockPrint",
    "flowersDaisies",
    "flowersModern1",
    "flowersModern2",
    "flowersPansy",
    "flowersRedRose",
    "flowersRoses",
    "flowersTeacup",
    "flowersTiny",
    "gems",
    "gingerbreadMan",
    "gradient",
    "handmade1",
    "handmade2",
    "heartBalloon",
    "heartGray",
    "hearts",
    "heebieJeebies",
    "holly",
    "houseFunky",
    "hypnotic",
    "iceCreamCones",
    "lightBulb",
    "lightning1",
    "lightning2",
    "mapPins",
    "mapleLeaf",
    "mapleMuffins",
    "marquee",
    "marqueeToothed",
    "moons",
    "mosaic",
    "musicNotes",
    "northwest",
    "ovals",
    "packages",
    "palmsBlack",
    "palmsColor",
    "paperClips",
    "papyrus",
    "partyFavor",
    "partyGlass",
    "pencils",
    "people",
    "peopleWaving",
    "peopleHats",
    "poinsettias",
    "postageStamp",
    "pumpkin1",
    "pushPinNote2",
    "pushPinNote1",
    "pyramids",
    "pyramidsAbove",
    "quadrants",
    "rings",
    "safari",
    "sawtooth",
    "sawtoothGray",
    "scaredCat",
    "seattle",
    "shadowedSquares",
    "sharksTeeth",
    "shorebirdTracks",
    "skyrocket",
    "snowflakeFancy",
    "snowflakes",
    "sombrero",
    "southwest",
    "stars",
    "starsTop",
    "stars3d",
    "starsBlack",
    "starsShadowed",
    "sun",
    "swirligig",
    "tornPaper",
    "tornPaperBlack",
    "trees",
    "triangleParty",
    "triangles",
    "triangle1",
    "triangle2",
    "triangleCircle1",
    "triangleCircle2",
    "shapes1",
    "shapes2",
    "twistedLines1",
    "twistedLines2",
    "vine",
    "waveline",
    "weavingAngles",
    "weavingBraid",
    "weavingRibbon",
    "weavingStrips",
    "whiteFlowers",
    "woodwork",
    "xIllusions",
    "zanyTriangles",
    "zigZag",
    "zigZagStitch",
    "custom",
];

/// `ST_TabJc` — Tab stop alignment type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ST_TabJc {
    Left,
    Center,
    Right,
    Decimal,
    Bar,
    Clear,
    Num,
    List,
    Start,
    End,
}

impl ST_TabJc {
    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "left" => Ok(Self::Left),
            "center" => Ok(Self::Center),
            "right" => Ok(Self::Right),
            "decimal" => Ok(Self::Decimal),
            "bar" => Ok(Self::Bar),
            "clear" => Ok(Self::Clear),
            "num" => Ok(Self::Num),
            "list" => Ok(Self::List),
            "start" => Ok(Self::Start),
            "end" => Ok(Self::End),
            _ => Err(OxmlError::InvalidValue(format!("invalid ST_TabJc: {s}"))),
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Center => "center",
            Self::Right => "right",
            Self::Decimal => "decimal",
            Self::Bar => "bar",
            Self::Clear => "clear",
            Self::Num => "num",
            Self::List => "list",
            Self::Start => "start",
            Self::End => "end",
        }
    }
}

/// `ST_TabTlc` — Tab leader character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ST_TabLeader {
    None,
    Dot,
    Hyphen,
    Underscore,
    Heavy,
    MiddleDot,
}

impl ST_TabLeader {
    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "none" => Ok(Self::None),
            "dot" => Ok(Self::Dot),
            "hyphen" => Ok(Self::Hyphen),
            "underscore" => Ok(Self::Underscore),
            "heavy" => Ok(Self::Heavy),
            "middleDot" => Ok(Self::MiddleDot),
            _ => Err(OxmlError::InvalidValue(format!(
                "invalid ST_TabLeader: {s}"
            ))),
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Dot => "dot",
            Self::Hyphen => "hyphen",
            Self::Underscore => "underscore",
            Self::Heavy => "heavy",
            Self::MiddleDot => "middleDot",
        }
    }
}

/// `ST_SectionType` — Section break type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ST_SectionType {
    NextPage,
    Continuous,
    EvenPage,
    OddPage,
    NextColumn,
}

impl ST_SectionType {
    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "nextPage" => Ok(Self::NextPage),
            "continuous" => Ok(Self::Continuous),
            "evenPage" => Ok(Self::EvenPage),
            "oddPage" => Ok(Self::OddPage),
            "nextColumn" => Ok(Self::NextColumn),
            _ => Err(OxmlError::InvalidValue(format!(
                "invalid ST_SectionType: {s}"
            ))),
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            Self::NextPage => "nextPage",
            Self::Continuous => "continuous",
            Self::EvenPage => "evenPage",
            Self::OddPage => "oddPage",
            Self::NextColumn => "nextColumn",
        }
    }
}

/// `ST_PageOrientation` — Page orientation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ST_PageOrientation {
    Portrait,
    Landscape,
}

impl ST_PageOrientation {
    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "portrait" => Ok(Self::Portrait),
            "landscape" => Ok(Self::Landscape),
            _ => Err(OxmlError::InvalidValue(format!(
                "invalid ST_PageOrientation: {s}"
            ))),
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            Self::Portrait => "portrait",
            Self::Landscape => "landscape",
        }
    }
}

/// `ST_HighlightColor` — Highlight colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ST_HighlightColor {
    Black,
    Blue,
    Cyan,
    DarkBlue,
    DarkCyan,
    DarkGray,
    DarkGreen,
    DarkMagenta,
    DarkRed,
    DarkYellow,
    Green,
    LightGray,
    Magenta,
    None,
    Red,
    White,
    Yellow,
}

impl ST_HighlightColor {
    pub fn from_str(s: &str) -> Result<Self> {
        match s {
            "black" => Ok(Self::Black),
            "blue" => Ok(Self::Blue),
            "cyan" => Ok(Self::Cyan),
            "darkBlue" => Ok(Self::DarkBlue),
            "darkCyan" => Ok(Self::DarkCyan),
            "darkGray" => Ok(Self::DarkGray),
            "darkGreen" => Ok(Self::DarkGreen),
            "darkMagenta" => Ok(Self::DarkMagenta),
            "darkRed" => Ok(Self::DarkRed),
            "darkYellow" => Ok(Self::DarkYellow),
            "green" => Ok(Self::Green),
            "lightGray" => Ok(Self::LightGray),
            "magenta" => Ok(Self::Magenta),
            "none" => Ok(Self::None),
            "red" => Ok(Self::Red),
            "white" => Ok(Self::White),
            "yellow" => Ok(Self::Yellow),
            _ => Err(OxmlError::InvalidValue(format!(
                "invalid ST_HighlightColor: {s}"
            ))),
        }
    }

    pub fn to_str(self) -> &'static str {
        match self {
            Self::Black => "black",
            Self::Blue => "blue",
            Self::Cyan => "cyan",
            Self::DarkBlue => "darkBlue",
            Self::DarkCyan => "darkCyan",
            Self::DarkGray => "darkGray",
            Self::DarkGreen => "darkGreen",
            Self::DarkMagenta => "darkMagenta",
            Self::DarkRed => "darkRed",
            Self::DarkYellow => "darkYellow",
            Self::Green => "green",
            Self::LightGray => "lightGray",
            Self::Magenta => "magenta",
            Self::None => "none",
            Self::Red => "red",
            Self::White => "white",
            Self::Yellow => "yellow",
        }
    }
}

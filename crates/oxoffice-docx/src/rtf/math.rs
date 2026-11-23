//! OfficeMath in RTF: the `\mmath` destination Word writes and reads, each
//! OMML element `m:X` spelled as a group `{\mX ...}`, a property's `m:val`
//! as the group's text (`{\mbegChr (}`), the text of a math run in
//! `{\mr ...}`. The spelling is Word's own (Word 16 "Save As RTF"), which
//! LibreOffice reads too.

use std::fmt::Write as _;

use oxoffice_docx_oxml::math::{CT_OMath, CT_OMathPara, OfficeMath};
use oxoffice_docx_oxml::namespace::{M_NS, W_NS};
use quick_xml::NsReader;
use quick_xml::events::Event;
use quick_xml::name::ResolveResult;

/// Properties whose value is on or off: `<m:degHide/>` and
/// `<m:degHide m:val="1"/>` are both written `{\mdegHide on}`.
const ON_OFF: &[&str] = &[
    "aln",
    "alnScr",
    "degHide",
    "diff",
    "grow",
    "hideBot",
    "hideLeft",
    "hideRight",
    "hideTop",
    "lit",
    "maxDist",
    "noBreak",
    "nor",
    "objDist",
    "opEmu",
    "plcHide",
    "show",
    "strikeBLTR",
    "strikeH",
    "strikeTLBR",
    "strikeV",
    "subHide",
    "supHide",
    "transp",
    "zeroAsc",
    "zeroDesc",
    "zeroWid",
];

/// Properties written as a group holding their value. Any other element
/// is a container (`{\mf ...}`), its children inside.
const VALUED: &[&str] = &[
    "argSz", "baseJc", "begChr", "cGp", "cGpRule", "chr", "count", "cSp", "endChr", "limLoc",
    "mcJc", "pos", "rSp", "rSpRule", "sepChr", "shp", "type", "vertJc",
];

/// An equation as OMML-in-RTF, and what the RTF cannot carry of it.
pub(super) struct RtfEquation {
    pub(super) rtf: String,
    pub(super) lost: Vec<String>,
}

/// The font Word writes math runs in.
pub(super) const MATH_FONT: &str = "Cambria Math";

/// Write `equation` as a `\mmath` group, its math runs in the font
/// `math_font` of the font table.
pub(super) fn write_equation(
    equation: &OfficeMath,
    math_font: Option<usize>,
) -> Result<RtfEquation, String> {
    let xml = equation.to_xml().map_err(|error| error.to_string())?;
    let mut reader = NsReader::from_reader(xml.as_slice());
    reader.config_mut().trim_text(false);
    let mut writer = Writer {
        math_font,
        ..Writer::default()
    };
    let mut buffer = Vec::new();
    loop {
        let event = reader
            .read_resolved_event_into(&mut buffer)
            .map_err(|error| error.to_string())?;
        match event {
            (ResolveResult::Bound(namespace), Event::Start(start)) => {
                let local = String::from_utf8_lossy(start.local_name().as_ref()).into_owned();
                let value = math_value(&start, &local);
                writer.start(namespace.as_ref(), &local, value, false);
            }
            (ResolveResult::Bound(namespace), Event::Empty(start)) => {
                let local = String::from_utf8_lossy(start.local_name().as_ref()).into_owned();
                let value = math_value(&start, &local);
                writer.start(namespace.as_ref(), &local, value, true);
            }
            (_, Event::End(_)) => writer.end(),
            (_, Event::Text(text)) => {
                let text = text.decode().map_err(|error| error.to_string())?;
                writer.text(&text);
            }
            (_, Event::GeneralRef(reference)) => {
                if let Some(character) = reference
                    .resolve_char_ref()
                    .map_err(|error| error.to_string())?
                {
                    writer.text(&character.to_string());
                } else {
                    let name = reference.decode().map_err(|error| error.to_string())?;
                    let value = quick_xml::escape::resolve_predefined_entity(&name)
                        .ok_or_else(|| format!("unknown entity &{name};"))?;
                    writer.text(value);
                }
            }
            (_, Event::CData(text)) => writer.text(&String::from_utf8_lossy(&text)),
            (_, Event::Eof) => break,
            _ => {}
        }
        buffer.clear();
    }
    let mut lost = writer.lost;
    lost.sort();
    lost.dedup();
    Ok(RtfEquation {
        rtf: format!("{{\\mmath{}}}", writer.out),
        lost,
    })
}

/// The value of an element: its `m:val`, a break's `m:alnAt`.
fn math_value(start: &quick_xml::events::BytesStart<'_>, local: &str) -> Option<String> {
    let name: &[u8] = if local == "brk" { b"alnAt" } else { b"val" };
    start.attributes().flatten().find_map(|attribute| {
        (attribute.key.local_name().as_ref() == name)
            .then(|| String::from_utf8_lossy(&attribute.value).into_owned())
    })
}

/// What an open element was written as.
enum Open {
    /// A group, closed at its end.
    Group,
    /// A math run, whose text is collected and written at its end.
    Run,
    /// Written whole at its start, or left out with its content.
    Nothing,
    /// Left out, its content kept (`w:ins`).
    Transparent,
}

#[derive(Default)]
struct Writer {
    out: String,
    open: Vec<Open>,
    /// The depth of a subtree being left out, while one is.
    skipping: Option<usize>,
    /// Whether the subtree left out is a math run's `w:rPr`.
    skipping_formatting: bool,
    /// The flags and text of the open math run.
    run: Option<(String, String)>,
    /// The math font's number in the font table.
    math_font: Option<usize>,
    lost: Vec<String>,
}

impl Writer {
    fn start(&mut self, namespace: &[u8], local: &str, value: Option<String>, empty: bool) {
        if self.skipping.is_some() {
            // The math font and the italic Word gives every math run are the
            // reader's own; other formatting (bold too) is not carried.
            if self.skipping_formatting
                && namespace == W_NS.as_bytes()
                && !matches!(local, "rFonts" | "i" | "iCs" | "lang")
            {
                self.lost
                    .push("character formatting of its math runs".to_owned());
            }
            if !empty {
                self.open.push(Open::Nothing);
            }
            return;
        }
        let open = self.element(namespace, local, value);
        if empty {
            if matches!(open, Open::Group) {
                self.out.push('}');
            }
            if let Open::Run = open {
                self.finish_run();
            }
        } else {
            if matches!(open, Open::Nothing) && self.skipping.is_none() {
                self.skipping = Some(self.open.len());
                self.skipping_formatting = namespace == W_NS.as_bytes() && local == "rPr";
            }
            self.open.push(open);
        }
    }

    /// Write the start of one element and say how it is written.
    fn element(&mut self, namespace: &[u8], local: &str, value: Option<String>) -> Open {
        if namespace == W_NS.as_bytes() {
            return match local {
                // The accepted view: a tracked insertion's content is kept,
                // a deletion's left out.
                "ins" | "moveTo" => Open::Transparent,
                "del" | "moveFrom" => Open::Nothing,
                // The run's character formatting, read as it is left out.
                "rPr" => Open::Nothing,
                _ => {
                    self.lost.push(format!("w:{local}"));
                    Open::Nothing
                }
            };
        }
        if namespace != M_NS.as_bytes() {
            self.lost
                .push(format!("content outside OfficeMath ({local})"));
            return Open::Nothing;
        }
        match local {
            "oMathPara" | "oMath" => {
                self.out.push_str(&format!("{{\\*\\m{local} "));
                Open::Group
            }
            // The control formatting of an object: the reader's own.
            "ctrlPr" => Open::Nothing,
            "r" => {
                self.run = Some((String::new(), String::new()));
                Open::Run
            }
            "rPr" if self.run.is_some() => Open::Transparent,
            "t" if self.run.is_some() => Open::Transparent,
            "sty" | "scr" | "nor" | "lit" if self.run.is_some() => {
                self.run_flag(local, value.as_deref());
                Open::Nothing
            }
            // A break, as Word writes it in the run: `m:alnAt="1"` is
            // `\mbrk0` (Word 16.115). No Word file shows the number of a break
            // without `m:alnAt` or aligned elsewhere: reported.
            "brk" if self.run.is_some() => {
                match (value.as_deref(), &mut self.run) {
                    (Some("1"), Some((flags, _))) => flags.push_str(RUN_BREAK),
                    (other, _) => self.lost.push(format!(
                        "m:brk of a math run (alnAt {})",
                        other.unwrap_or("none")
                    )),
                }
                Open::Nothing
            }
            // The other run properties (`m:aln`): Word draws no alignment
            // from it (it aligns on `&` in the text) and no Word file shows
            // its spelling, so it is reported rather than written where no
            // reader looks for it.
            local if self.run.is_some() => {
                self.lost.push(format!("m:{local} of a math run"));
                Open::Nothing
            }
            "jc" => {
                // Word writes a justification as a number; only these two are
                // seen in its files.
                match value.as_deref() {
                    Some("centerGroup") => self.out.push_str("\\mjc1 "),
                    Some("center") => self.out.push_str("\\mjc2 "),
                    other => self
                        .lost
                        .push(format!("justification {}", other.unwrap_or("(none)"))),
                }
                Open::Nothing
            }
            local if ON_OFF.contains(&local) => {
                let on = !matches!(value.as_deref(), Some("0" | "false" | "off"));
                self.out
                    .push_str(&format!("{{\\m{local} {}", if on { "on" } else { "off" }));
                Open::Group
            }
            local if VALUED.contains(&local) => {
                self.out.push_str(&format!("{{\\m{local} "));
                push_rtf_text(&mut self.out, value.as_deref().unwrap_or_default());
                Open::Group
            }
            // A value the reader would not read back as one: reported, so
            // that writer and reader always agree on what is a value.
            local if value.is_some() => {
                self.lost.push(format!("m:{local}"));
                Open::Nothing
            }
            local => {
                self.out.push_str(&format!("{{\\m{local}"));
                Open::Group
            }
        }
    }

    /// A flag of the open math run, as Word spells it: `\mscr0` for the
    /// roman script, `\msty0` plain and `\msty2` italic. The other scripts
    /// and styles are left out: no Word file at hand shows their numbers.
    /// A style left out is reported, and its run written without a style.
    fn run_flag(&mut self, local: &str, value: Option<&str>) {
        let flag = match (local, value) {
            ("sty", Some("p")) => "\\msty0",
            ("sty", Some("i")) => "\\msty2",
            ("scr", Some("roman")) => "\\mscr0",
            ("nor", _) => "\\mnor",
            ("lit", _) => "\\mlit",
            (local, value) => {
                self.lost
                    .push(format!("{local} {}", value.unwrap_or("(none)")));
                if local == "sty"
                    && let Some((flags, _)) = &mut self.run
                {
                    flags.push_str(STYLE_LEFT_OUT);
                }
                return;
            }
        };
        if let Some((flags, _)) = &mut self.run {
            flags.push_str(flag);
        }
    }

    fn finish_run(&mut self) {
        if let Some((flags, text)) = self.run.take() {
            // The break comes after the script and style, as Word writes it.
            let has_break = flags.contains(RUN_BREAK);
            let mut flags = flags.replace(RUN_BREAK, "");
            // Word writes every run with its script and style, the default
            // one as `\mscr0\msty2`, and reads a run without `\msty` as
            // plain: a run of the default style says so.
            if !flags.contains("\\mnor") {
                if !flags.contains("\\mscr") {
                    flags.insert_str(0, "\\mscr0");
                }
                if !flags.contains("\\msty") && !flags.contains(STYLE_LEFT_OUT) {
                    flags.push_str("\\msty2");
                }
            }
            let mut flags = flags.replace(STYLE_LEFT_OUT, "");
            if has_break {
                flags.push_str(RUN_BREAK);
            }
            // Word wraps each math run in its character formatting: the
            // math font, and italic on a run of the default style (`\msty2`,
            // letters, Greek, digits and operators alike). Word reads a run
            // without it as plain. A normal-text run (`\mnor`) keeps the
            // body font.
            let formatting = match self.math_font {
                Some(font) if !flags.contains("\\mnor") => Some(format!(
                    "{{{}\\f{font} ",
                    if flags.contains("\\msty2") { "\\i" } else { "" }
                )),
                _ => None,
            };
            if let Some(formatting) = &formatting {
                self.out.push_str(formatting);
            }
            self.out.push_str(&format!("{{\\mr{flags} "));
            push_rtf_text(&mut self.out, &text);
            self.out.push('}');
            if formatting.is_some() {
                self.out.push('}');
            }
        }
    }

    fn text(&mut self, text: &str) {
        if self.skipping.is_some() {
            return;
        }
        if let Some((_, collected)) = &mut self.run {
            collected.push_str(text);
        } else if !text.trim().is_empty() {
            self.lost.push("text outside a math run".to_owned());
        }
    }

    fn end(&mut self) {
        let Some(open) = self.open.pop() else {
            return;
        };
        if self.skipping == Some(self.open.len()) {
            self.skipping = None;
            return;
        }
        if self.skipping.is_some() {
            return;
        }
        match open {
            Open::Group => self.out.push('}'),
            Open::Run => self.finish_run(),
            Open::Nothing | Open::Transparent => {}
        }
    }
}

/// `text` in RTF: `\`, `{` and `}` escaped, other than ASCII as `\uN?`.
fn push_rtf_text(out: &mut String, text: &str) {
    for scalar in text.chars() {
        match scalar {
            '\\' | '{' | '}' => {
                out.push('\\');
                out.push(scalar);
            }
            '\r' | '\n' => {}
            scalar if scalar.is_ascii() && !scalar.is_ascii_control() => out.push(scalar),
            scalar if scalar.is_control() => {}
            scalar => {
                let mut units = [0_u16; 2];
                for unit in scalar.encode_utf16(&mut units) {
                    let signed = i16::from_ne_bytes(unit.to_ne_bytes()) as i32;
                    let _ = write!(out, "\\u{signed}?");
                }
            }
        }
    }
}

/// An equation read back from a `\mmath` group, built as OMML.
#[derive(Default)]
pub(super) struct MathBuilder {
    xml: String,
    /// What each open group of the destination is.
    frames: Vec<Frame>,
    /// Whether the outermost element was a display group.
    display: bool,
    /// The text of its runs, in order: what a reader without math shows,
    /// kept when the equation cannot be rebuilt.
    pub(super) text: String,
    pub(super) lost: Vec<String>,
}

enum Frame {
    /// A group not yet named, or one that holds only formatting and text.
    Plain(String),
    /// An element whose children follow (`{\mf ...}`).
    Element(String),
    /// A property whose text is its value (`{\mchr ...}`).
    Valued(String, String),
    /// A math run: its flags and text.
    Run(String, String),
    /// A group left out with its content (`\mmathPict`, `\mctrlPr`).
    Skipped,
}

impl MathBuilder {
    /// A group opens inside the destination.
    pub(super) fn open(&mut self) {
        if matches!(self.frames.last(), Some(Frame::Skipped)) {
            self.frames.push(Frame::Skipped);
        } else {
            self.frames.push(Frame::Plain(String::new()));
        }
    }

    /// A control word inside the destination: it names the group it starts
    /// (`at_start`) or sets a flag of the group.
    pub(super) fn word(&mut self, name: &str, parameter: Option<i32>, at_start: bool) {
        let Some(frame) = self.frames.last_mut() else {
            return;
        };
        if matches!(frame, Frame::Skipped) {
            return;
        }
        let Some(local) = name.strip_prefix('m') else {
            // Character formatting (`\i`, `\f34`): the math font is the
            // reader's own.
            return;
        };
        if at_start && matches!(frame, Frame::Plain(text) if text.is_empty()) {
            // OMML allows an alignment point and a break as elements of their
            // own only in a box's properties; a run's are flags of the run
            // (`\mbrk0`). Elsewhere they would make the OMML invalid.
            let misplaced = matches!(local, "aln" | "brk") && !self.in_box_properties();
            let Some(frame) = self.frames.last_mut() else {
                return;
            };
            *frame = match local {
                "mathPict" | "ctrlPr" | "mathPr" => Frame::Skipped,
                _ if misplaced => {
                    self.lost.push(format!("\\m{local}"));
                    Frame::Skipped
                }
                "r" => Frame::Run(String::new(), String::new()),
                local if ON_OFF.contains(&local) || VALUED.contains(&local) => {
                    Frame::Valued(local.to_owned(), String::new())
                }
                local => {
                    if local == "oMathPara" && self.xml.is_empty() {
                        self.display = true;
                    }
                    let _ = write!(self.xml, "<m:{local}>");
                    Frame::Element(local.to_owned())
                }
            };
            return;
        }
        match (frame, local, parameter) {
            (Frame::Run(flags, _), "sty", Some(0)) => flags.push_str(r#"<m:sty m:val="p"/>"#),
            // Word writes its default style, italic letters and upright
            // digits and operators, as `\msty2` on every such run.
            (Frame::Run(flags, _), "sty", Some(2)) => flags.push_str(DEFAULT_STYLE),
            // The roman script is OMML's default, which Word writes on every
            // run as `\mscr0`: no `m:scr`.
            (Frame::Run(..), "scr", Some(0)) => {}
            (Frame::Run(flags, _), "nor", _) => flags.push_str("<m:nor/>"),
            (Frame::Run(flags, _), "lit", _) => flags.push_str("<m:lit/>"),
            // Word writes a break aligned at the first operator as `\mbrk0`.
            (Frame::Run(flags, _), "brk", Some(0)) => flags.push_str(READ_BREAK),
            (_, "jc", Some(1)) => self.xml.push_str(r#"<m:jc m:val="centerGroup"/>"#),
            (_, "jc", Some(2)) => self.xml.push_str(r#"<m:jc m:val="center"/>"#),
            (_, "mathFont" | "brkBin" | "brkBinSub", _) => {}
            (_, local, parameter) => self.lost.push(match parameter {
                Some(parameter) => format!("\\m{local}{parameter}"),
                None => format!("\\m{local}"),
            }),
        }
    }

    /// Whether the group being named is in a box's properties (`m:boxPr`),
    /// past any formatting group around it.
    fn in_box_properties(&self) -> bool {
        let parents = &self.frames[..self.frames.len().saturating_sub(1)];
        parents
            .iter()
            .rev()
            .find(|frame| !matches!(frame, Frame::Plain(text) if text.is_empty()))
            .is_some_and(|frame| matches!(frame, Frame::Element(local) if local == "boxPr"))
    }

    /// Text inside the destination.
    pub(super) fn text(&mut self, text: &str) {
        match self.frames.last_mut() {
            Some(
                Frame::Plain(collected) | Frame::Valued(_, collected) | Frame::Run(_, collected),
            ) => collected.push_str(text),
            Some(Frame::Element(_)) if !text.trim().is_empty() => {
                // Text straight in an element is a run of its own, as Word
                // writes a comma between two runs.
                write_run(&mut self.xml, "", text);
                self.text.push_str(text);
            }
            _ => {}
        }
    }

    /// A group of the destination closes.
    pub(super) fn close(&mut self) {
        match self.frames.pop() {
            Some(Frame::Element(local)) => {
                // An element with no content is written empty (`<m:deg/>`).
                let open = format!("<m:{local}>");
                if self.xml.ends_with(&open) {
                    self.xml.truncate(self.xml.len() - open.len());
                    let _ = write!(self.xml, "<m:{local}/>");
                } else {
                    let _ = write!(self.xml, "</m:{local}>");
                }
            }
            Some(Frame::Valued(local, value)) => {
                let value = value.trim_start();
                // An on-off property that is on is written as OMML writes it
                // by default, with no value (`<m:aln/>`).
                if value == "on" && ON_OFF.contains(&local.as_str()) {
                    let _ = write!(self.xml, "<m:{local}/>");
                } else {
                    let _ = write!(
                        self.xml,
                        r#"<m:{local} m:val="{}"/>"#,
                        escape_attribute(value)
                    );
                }
            }
            Some(Frame::Run(flags, text)) => {
                // The break follows the style in OMML.
                let has_break = flags.contains(READ_BREAK);
                let flags = flags.replace(READ_BREAK, "");
                // Word reads a run without `\msty` as plain.
                let mut flags = if flags.contains("<m:sty") || flags.contains("<m:nor/>") {
                    flags.replace(DEFAULT_STYLE, "")
                } else {
                    format!(r#"{flags}<m:sty m:val="p"/>"#)
                };
                if has_break {
                    flags.push_str(READ_BREAK);
                }
                write_run(&mut self.xml, &flags, &text);
                self.text.push_str(&text);
            }
            Some(Frame::Plain(text)) if !text.is_empty() => {
                write_run(&mut self.xml, "", &text);
                self.text.push_str(&text);
            }
            _ => {}
        }
    }

    /// The equation, once the destination is closed.
    pub(super) fn finish(self) -> Result<(OfficeMath, Vec<String>), String> {
        let xml = format!(
            r#"<m:root xmlns:m="{M_NS}" xmlns:w="{W_NS}">{}</m:root>"#,
            self.xml
        );
        // The root element of the equation, with the namespaces declared.
        let inner = xml
            .strip_prefix(&format!(r#"<m:root xmlns:m="{M_NS}" xmlns:w="{W_NS}">"#))
            .and_then(|rest| rest.strip_suffix("</m:root>"))
            .unwrap_or_default();
        let declared = |tag: &str| {
            inner.replacen(
                &format!("<m:{tag}>"),
                &format!(r#"<m:{tag} xmlns:m="{M_NS}" xmlns:w="{W_NS}">"#),
                1,
            )
        };
        let equation = if self.display {
            CT_OMathPara::from_xml(declared("oMathPara").as_bytes()).map(OfficeMath::Display)
        } else {
            CT_OMath::from_xml(declared("oMath").as_bytes()).map(OfficeMath::Inline)
        }
        .map_err(|error| error.to_string())?;
        Ok((equation, self.lost))
    }
}

/// A math run's break aligned at the first operator, as Word writes it.
const RUN_BREAK: &str = "\\mbrk0";

/// [`RUN_BREAK`] read back as OMML.
const READ_BREAK: &str = r#"<m:brk m:alnAt="1"/>"#;

/// The mark of a run's style the writer leaves out (reported).
const STYLE_LEFT_OUT: &str = "\u{1}";

/// The mark of `\msty2`, Word's default style: no `m:sty` in OMML.
const DEFAULT_STYLE: &str = "<m:sty/>";

fn write_run(xml: &mut String, flags: &str, text: &str) {
    if text.is_empty() {
        return;
    }
    let properties = if flags.is_empty() {
        String::new()
    } else {
        format!("<m:rPr>{flags}</m:rPr>")
    };
    let _ = write!(
        xml,
        r#"<m:r>{properties}<m:t xml:space="preserve">{}</m:t></m:r>"#,
        escape_text(text)
    );
}

fn escape_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_attribute(text: &str) -> String {
    escape_text(text).replace('"', "&quot;")
}

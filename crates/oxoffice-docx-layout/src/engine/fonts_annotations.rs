//! Fonts and languages by script, emphasis marks, ruby, East Asian layout and run colour.

use super::*;

/// One of the four `w:rFonts` script slots, as this engine resolves them.
///
/// `oxoffice_docx::RunFontSlot` is the same four slots on the authoring side, but
/// `oxoffice-docx-layout` sits below `oxoffice-docx` and cannot name it. The shape here
/// deliberately mirrors `WordLanguageSlot` next to it, so a reader meets one
/// pattern rather than two.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordFontSlot {
    /// ASCII characters using the `w:ascii` slot.
    Ascii,
    /// Other Latin characters using the `w:hAnsi` slot.
    HighAnsi,
    /// East Asian characters using the `w:eastAsia` slot.
    EastAsia,
    /// Complex-script characters using the `w:cs` slot.
    ComplexScript,
}

/// Word's East Asian codepoint set.
///
/// Three tables ask this question, the font slot, the language slot and the
/// rich-path gate, and asking it three times is how they drifted apart. They
/// now ask it once. It is a superset of every codepoint `script_for_char`
/// calls Hangul, Kana or Han, because Word draws whole CJK-adjacent blocks
/// from `w:eastAsia` that need no complex shaping of their own.
pub(super) fn is_east_asian(codepoint: u32) -> bool {
    matches!(
        codepoint,
        // Hangul jamo, including Extended-A and Extended-B, and the
        // precomposed syllables and halfwidth jamo.
        0x1100..=0x11ff
            | 0xa960..=0xa97f
            | 0xac00..=0xd7ff
            // CJK radicals, Kangxi radicals and the ideographic description
            // characters.
            | 0x2e80..=0x2eff
            | 0x2f00..=0x2fdf
            | 0x2ff0..=0x2fff
            // CJK punctuation, Kana, Bopomofo, Kanbun, the CJK strokes and
            // the enclosed and compatibility blocks.
            | 0x3000..=0x33ff
            // Han, its extension A, the compatibility ideographs and its
            // supplementary-plane extensions.
            | 0x3400..=0x9fff
            | 0xf900..=0xfaff
            | 0x20000..=0x2fa1f
            // CJK compatibility forms, small form variants, and the
            // halfwidth and fullwidth forms.
            | 0xfe30..=0xfe6f
            | 0xff00..=0xffef
    )
}

/// The `w:rFonts` slot one character resolves through.
///
/// The slot boundaries are not the [`word_language_slot`] boundaries. Word
/// routes Devanagari and Thai through `w:cs` while their language still comes
/// from `w:lang/@w:val`, which is why these are two tables and not one.
///
/// A character whose codepoint settles its own script decides its own slot and
/// ignores `w:rFonts/@w:hint`. Everything else is Word's ambiguous set, the
/// punctuation, symbols, digits, Greek and Cyrillic that belong to no script
/// in particular, and there the hint decides. That is the whole reason the
/// attribute exists, so a table that classified the ambiguous set by codepoint
/// would leave the hint inert on exactly the characters it is written for.
fn word_font_slot(character: char, hint: Option<&str>) -> WordFontSlot {
    let codepoint = character as u32;
    match codepoint {
        // Hebrew, Arabic, Syriac, Thaana and the Arabic presentation forms,
        // plus the Devanagari and Thai ranges Word also draws from `w:cs`.
        0x0590..=0x08ff | 0x0900..=0x097f | 0x0e00..=0x0e7f | 0xfb1d..=0xfdff | 0xfe70..=0xfeff => {
            WordFontSlot::ComplexScript
        }
        // Every CJK-adjacent block, which Word draws from `w:eastAsia`.
        codepoint if is_east_asian(codepoint) => WordFontSlot::EastAsia,
        // The Latin letters Word draws from `w:ascii`.
        0x0041..=0x005a | 0x0061..=0x007a => WordFontSlot::Ascii,
        // The Latin supplement and extensions, the IPA extensions and
        // spacing modifier letters, Latin Extended-D and the Latin ligatures,
        // which Word draws from `w:hAnsi`. The IPA and modifier blocks are
        // Latin script by Unicode even though this workspace's shaping table
        // stops at U+024F, so their slot is not in doubt either.
        0x00c0..=0x02ff | 0x1e00..=0x1eff | 0xa720..=0xa7ff | 0xfb00..=0xfb1c => {
            WordFontSlot::HighAnsi
        }
        // Word's ambiguous set. Only here does `w:hint` decide, and without
        // one the seven-bit range keeps `w:ascii` and everything above it
        // takes `w:hAnsi`, which is what each did before slots existed.
        _ => match hint {
            Some("eastAsia") => WordFontSlot::EastAsia,
            Some("cs") => WordFontSlot::ComplexScript,
            _ if codepoint <= 0x007f => WordFontSlot::Ascii,
            _ => WordFontSlot::HighAnsi,
        },
    }
}

/// The `w:rFonts` slot a whole run resolves through.
///
/// One family is resolved per run, because `resolve_font_for_text` already
/// picks one face for the run's entire text and replaces it wholesale when it
/// cannot draw the run.
///
/// Only an alphabetic character claims a slot. Spaces, digits and punctuation
/// take whatever the letters take, so they do not decide it while a letter is
/// present. A run with no letter at all has nothing to follow, so its own
/// characters decide instead, which is how a run of East Asian punctuation or
/// fullwidth digits reaches `w:eastAsia`. A run of exactly that shape is
/// ordinary in Japanese and Chinese prose, because Word splits runs at
/// formatting, proofing and revision boundaries.
///
/// Both passes take the same consensus rule, so a run's answer never turns on
/// which character the author typed first. Any disagreement resolves to
/// `w:ascii`, because preferring either side would be wrong for the other and
/// `w:ascii` is what such a run resolved through before any slot existed. A
/// run of ASCII therefore resolves exactly as it always has.
///
/// The second pass drops its `w:ascii` candidates before counting them, since
/// `w:ascii` is also the answer a disagreement gives, so counting them would
/// make every run of ideographic punctuation around a space answer `w:ascii`
/// twice over.
pub(super) fn word_font_slot_for_text(text: &str, hint: Option<&str>) -> WordFontSlot {
    /// The one slot every candidate agrees on, `Ascii` if they disagree, or
    /// None if there was no candidate. Generic over the iterator because it is
    /// called with two different ones just below.
    fn consensus(slots: impl Iterator<Item = WordFontSlot>) -> Option<WordFontSlot> {
        let mut claimed: Option<WordFontSlot> = None;
        for slot in slots {
            match claimed {
                None => claimed = Some(slot),
                Some(existing) if existing == slot => {}
                Some(_) => return Some(WordFontSlot::Ascii),
            }
        }
        claimed
    }

    if let Some(slot) = consensus(
        text.chars()
            .filter(|character| character.is_alphabetic())
            .map(|character| word_font_slot(character, hint)),
    ) {
        return slot;
    }

    // `w:ascii` candidates are dropped rather than counted, because `w:ascii`
    // is also the answer a disagreement gives, so counting them would make
    // every mixed run of punctuation answer `w:ascii` twice over.
    consensus(
        text.chars()
            .map(|character| word_font_slot(character, hint))
            .filter(|slot| *slot != WordFontSlot::Ascii),
    )
    .unwrap_or(WordFontSlot::Ascii)
}

/// The explicit `w:rFonts` family for one slot.
///
/// Word falls back to the `w:ascii` family when the slot the character wants
/// is absent, which is also what keeps an `ascii`-only run resolving the way
/// it always has.
fn word_font_for_slot(
    rpr: &oxoffice_docx_oxml::properties::CT_RPr,
    theme: Option<&oxoffice_docx_oxml::theme::Theme>,
    slot: WordFontSlot,
) -> Option<String> {
    // Explicit font name takes priority over the theme attribute beside it.
    //
    // Word prefers the theme attribute when a producer presents both for one
    // slot, so this is a deliberate divergence, pre-declared under rule 5 of
    // .claude/skills/differential-testing.md. A document authored through this
    // facade never presents both for one slot, because setting either form
    // clears the other, so the divergence is reachable only on a producer
    // document the caller never edited. Leaving those bytes as written is what
    // the no-op save contract requires.
    let explicit = match slot {
        WordFontSlot::Ascii => &rpr.font_ascii,
        WordFontSlot::HighAnsi => &rpr.font_hansi,
        WordFontSlot::EastAsia => &rpr.font_east_asia,
        WordFontSlot::ComplexScript => &rpr.font_cs,
    };
    if explicit.is_some() {
        return explicit.clone();
    }

    // Each slot reads its own theme attribute. Reading `w:asciiTheme` for
    // every slot made `w:eastAsiaTheme` and `w:cstheme` inert and collapsed
    // majorEastAsia, minorEastAsia, majorBidi and minorBidi onto whatever the
    // ASCII slot named.
    let theme_ref = match slot {
        WordFontSlot::Ascii => &rpr.font_ascii_theme,
        WordFontSlot::HighAnsi => &rpr.font_hansi_theme,
        WordFontSlot::EastAsia => &rpr.font_east_asia_theme,
        WordFontSlot::ComplexScript => &rpr.font_cs_theme,
    }
    .as_deref()?;
    let theme = theme?;
    match theme_ref {
        // These four name the theme's `a:latin` typeface, which is the one
        // `oxoffice_docx_oxml::theme::Theme` carries.
        "majorAscii" | "majorHAnsi" => theme.major_font.as_deref(),
        "minorAscii" | "minorHAnsi" => theme.minor_font.as_deref(),
        // majorEastAsia, minorEastAsia, majorBidi and minorBidi name the
        // theme's `a:ea` and `a:cs` typefaces. `Theme` models neither, so
        // there is no entry to read and the answer here is None, which falls
        // through to the run's own `w:ascii` family. Word behaves the same
        // way, since `a:cs` is empty in every stock Office theme.
        //
        // Declining *here* is what stops a Latin face, which usually cannot
        // draw the text, outranking the family the author named.
        // `resolve_font_family` answers with that face only after both slots
        // have declined, where there is nothing else left to answer with.
        _ => None,
    }
    .map(str::to_owned)
}

/// Resolve a font family from already effective, cascaded run properties.
///
/// Callers must apply document defaults, style inheritance and direct run
/// properties before calling this function. It resolves font slots and theme
/// references only and does not perform that cascade itself.
///
/// Five steps, in order. The slot's explicit family, the slot's theme font,
/// then the same two for the `w:ascii` slot, then the Latin typeface for a
/// non-Latin theme reference as a last resort, then None so the default
/// applies.
///
/// The character's own slot is resolved completely before the `w:ascii`
/// fallback, because returning the explicit `w:ascii` family first would leave
/// `w:eastAsiaTheme` and `w:cstheme` exactly as inert as they were before they
/// were read at all.
///
/// The last resort exists because `majorEastAsia`, `minorEastAsia`,
/// `majorBidi` and `minorBidi` decline inside a slot, and a run whose only
/// font property is one of those four would otherwise lose its theme typeface
/// entirely. Declining inside the slot and answering outside it is what makes
/// the Latin face reachable without ever letting it outrank a family the
/// author named.
pub fn resolve_font_family(
    rpr: &oxoffice_docx_oxml::properties::CT_RPr,
    theme: Option<&oxoffice_docx_oxml::theme::Theme>,
    slot: WordFontSlot,
) -> Option<String> {
    if let Some(family) = word_font_for_slot(rpr, theme, slot)
        .or_else(|| word_font_for_slot(rpr, theme, WordFontSlot::Ascii))
    {
        return Some(family);
    }

    // Last resort. The four non-Latin references decline above so they can
    // never outrank a family the author named, but a run whose only font
    // property is one of them has nothing else to fall back to, and the Latin
    // typeface is a better answer for it than the engine default. This is
    // reachable only from a producer document, since Word writes those four
    // on `w:eastAsiaTheme` and `w:cstheme` where the run's own `w:ascii`
    // family answers first.
    let theme = theme?;
    let reference = match slot {
        WordFontSlot::Ascii => &rpr.font_ascii_theme,
        WordFontSlot::HighAnsi => &rpr.font_hansi_theme,
        WordFontSlot::EastAsia => &rpr.font_east_asia_theme,
        WordFontSlot::ComplexScript => &rpr.font_cs_theme,
    }
    .as_deref()
    .or(rpr.font_ascii_theme.as_deref())?;
    match reference {
        "majorEastAsia" | "majorBidi" => theme.major_font.as_deref(),
        "minorEastAsia" | "minorBidi" => theme.minor_font.as_deref(),
        _ => None,
    }
    .map(str::to_owned)
}

/// East Asian annotation, the shared geometry behind `w:em` and `w:ruby`.
///
/// Both place a second, smaller line against a base line without changing
/// the base advance, so both are measured as one `InlineItem::Group` whose
/// width is the base width and whose ascent already includes the annotation.
/// `item_metrics` turns that ascent into line height, which is how a marked
/// or annotated line becomes taller and reaches the paginator.
///
/// The mark and phonetic glyphs are painted inside the group rather than
/// carried on the text segment, because a segment field would have to be
/// sliced in step with glyph clusters at every line break and under bidi
/// reordering, which is exactly the case count the structural rules reject.
/// The cost is that the annotated text is one unbreakable item, so an
/// emphasis-marked run is split at whitespace first to keep its break
/// opportunities.
const EMPHASIS_MARK_SCALE: f64 = 0.5;

/// The glyph Word draws for one `w:em` value, and whether it sits above the
/// base characters rather than below them.
pub(super) fn emphasis_mark_glyph(mark: &ST_Em) -> Option<(char, bool)> {
    match mark {
        // U+2022 BULLET for the solid dot Word draws above the character,
        // U+FE45 SESAME DOT for the comma mark, U+25CB WHITE CIRCLE for the
        // open circle. `underDot` is the same solid dot below the base line.
        ST_Em::Dot => Some(('\u{2022}', true)),
        ST_Em::Comma => Some(('\u{FE45}', true)),
        ST_Em::Circle => Some(('\u{25CB}', true)),
        ST_Em::UnderDot => Some(('\u{2022}', false)),
        // `none` draws nothing, and a producer token outside the inventory
        // names a mark this renderer has no glyph for.
        ST_Em::None | ST_Em::Other(_) => None,
    }
}

/// Split text into maximal whitespace and non-whitespace chunks.
fn emphasis_chunks(text: &str) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut start = 0usize;
    let mut current = None;
    for (index, character) in text.char_indices() {
        let is_space = character.is_whitespace();
        match current {
            Some(previous) if previous == is_space => {}
            Some(_) => {
                chunks.push(&text[start..index]);
                start = index;
            }
            None => start = index,
        }
        current = Some(is_space);
    }
    if start < text.len() {
        chunks.push(&text[start..]);
    }
    chunks
}

/// The bracket pair `w:combineBrackets` names, drawn around a combined run.
///
/// The ASCII pair is deliberate. Every bundled face carries it, so a combined
/// run draws the same brackets on every host, which a fullwidth pair present
/// in only some East Asian faces would not.
fn combine_bracket_pair(brackets: Option<&str>) -> (&'static str, &'static str) {
    match brackets {
        Some("round") => ("(", ")"),
        Some("square") => ("[", "]"),
        Some("angle") => ("<", ">"),
        Some("curly") => ("{", "}"),
        _ => ("", ""),
    }
}

fn annotation_recipe(
    kind: WordAnnotationKind,
    base: &AnnotationBase<'_>,
    source_span: Option<SourceSpan>,
) -> Result<Option<Arc<WordAnnotationRecipe>>> {
    let Some(occurrence) = base.occurrence else {
        return Ok(None);
    };
    let color = |color: Color| {
        [
            color.r.to_bits(),
            color.g.to_bits(),
            color.b.to_bits(),
            color.a.to_bits(),
        ]
    };
    if [base.font_size, base.baseline_offset, base.spacing]
        .iter()
        .any(|value| !value.is_finite())
    {
        return Err(LayoutError::Layout(
            "nonfinite Word annotation producer".into(),
        ));
    }
    let font_family = base
        .font_family
        .map(|family| {
            let mut value = String::new();
            value
                .try_reserve_exact(family.len())
                .map_err(|_| LayoutError::Layout("annotation family allocation refused".into()))?;
            value.push_str(family);
            Ok(value)
        })
        .transpose()?;
    Ok(Some(Arc::new(WordAnnotationRecipe {
        occurrence,
        fallback_emphasis: if matches!(kind, WordAnnotationKind::EastAsian(_)) {
            base.fallback_emphasis.cloned()
        } else {
            None
        },
        field_kind: base.field_kind,
        source_span,
        kind,
        style: WordAnnotationStyle {
            font_id: base.font_id,
            font_size: base.font_size.to_bits(),
            color: color(base.color),
            bold: base.bold,
            italic: base.italic,
            baseline_offset: base.baseline_offset.to_bits(),
            spacing: base.spacing.to_bits(),
            underline: base.underline,
            strike: base.strike,
            dstrike: base.dstrike,
            highlight: base.highlight.map(color),
            font_family,
            kern: base.kern,
        },
    })))
}

pub(super) fn annotation_field_owner(base: &AnnotationBase<'_>) -> FieldPaintOwner {
    base.occurrence
        .map_or(FieldPaintOwner::Neutral, |occurrence| {
            FieldPaintOwner::Word {
                kern: base.kern,
                legacy: true,
                occurrence: Some(occurrence),
            }
        })
}

fn register_inline_annotation(
    styles: &mut HashMap<usize, WordMultilingualStyle>,
    slot: usize,
    recipe: Option<Arc<WordAnnotationRecipe>>,
    policies: PaintPolicies,
    kind: WordAnnotationRootKind,
) {
    if let Some(recipe) = recipe {
        styles.insert(
            slot,
            WordMultilingualStyle {
                kern: recipe.style.kern,
                origin: WordInputOrigin::Annotation,
                occurrence: Some(recipe.occurrence),
                annotation: Some(WordInputAnnotation {
                    slot,
                    recipe,
                    policies,
                    kind,
                }),
                ..Default::default()
            },
        );
    }
}

pub(super) fn push_empty_field_annotation(
    items: &mut Vec<InlineItem>,
    styles: &mut HashMap<usize, WordMultilingualStyle>,
    kind: WordAnnotationKind,
    base: &AnnotationBase<'_>,
    source: Option<SourceSpan>,
) -> Result<()> {
    let recipe = annotation_recipe(kind, base, source)?;
    let occurrence = base.occurrence.ok_or_else(|| {
        LayoutError::Layout("empty annotation lacks its known field owner".into())
    })?;
    let mut children = PaintedElements::default();
    children.push_field(
        PositionedElement::Text(GlyphRun {
            origin: Point { x: 0.0, y: 0.0 },
            font_id: base.font_id,
            font_size: base.font_size,
            glyph_ids: Vec::new(),
            advances: Vec::new(),
            text: String::new(),
            source,
            color: base.color,
            bold: base.bold,
            italic: base.italic,
            field_kind: base.field_kind,
            field_source: Some(occurrence.source),
            note: None,
            note_reference_source: None,
            tab_aligned: None,
        }),
        annotation_field_owner(base),
    )?;
    let (children, policies) = children.into_parts()?;
    register_inline_annotation(
        styles,
        items.len(),
        recipe,
        policies,
        WordAnnotationRootKind::EmptySpan,
    );
    items.push(InlineItem::Group {
        width: 0.0,
        height: 0.0,
        baseline: Some(0.0),
        math: false,
        group: GroupElement {
            transform: Transform::IDENTITY,
            clip: None,
            opacity: 1.0,
            effects: Vec::new(),
            children,
        },
    });
    Ok(())
}

/// Lay one run out under `w:eastAsianLayout`, or report that it does not apply.
///
/// `w:combine` compresses the run into one base-character advance inside the
/// bracket pair `w:combineBrackets` names. `w:vert` rotates the run 90 degrees
/// within its line, and `w:vertCompress` narrows the rotated run to one base
/// advance. A run that sets neither returns `false` and takes the ordinary
/// path. A run that sets this and `w:em` takes this, because combining and
/// rotating change the run's advance while an emphasis mark decorates it.
#[allow(clippy::too_many_arguments)]
pub(super) fn push_east_asian_layout_text(
    inline_items: &mut Vec<InlineItem>,
    styles: &mut HashMap<usize, WordMultilingualStyle>,
    fm: &mut FontManager,
    layout: &CT_EastAsianLayout,
    text: &str,
    base: &AnnotationBase<'_>,
    source: Option<SourceSpan>,
) -> Result<bool> {
    let recipe = annotation_recipe(WordAnnotationKind::EastAsian(layout.clone()), base, source)?;
    let combine = layout.combine == Some(true);
    let vert = layout.vert == Some(true);
    if !combine && !vert {
        return Ok(false);
    }
    let metrics = requested_metrics(fm, base)?;
    let ascent = metrics.ascent.max(0.0);
    let descent = metrics.descent.max(0.0);
    let mut shaped = fm.shape_text_with_kerning(base.font_id, text, base.font_size, base.kern)?;
    if base.spacing != 0.0 {
        for advance in &mut shaped.advances {
            *advance += base.spacing;
        }
        shaped.width += base.spacing * shaped.advances.len() as f64;
    }
    if shaped.width <= 0.0 {
        return Ok(false);
    }

    // One base-character advance is one em of the run's own size, which is
    // the character cell `w:combine` fits a run into and the advance
    // `w:vertCompress` narrows a rotated run to. It is deliberately not the
    // run's first shaped advance: shaping returns visual order, so a
    // right-to-left run's first advance is its logically last character, and a
    // run that begins with a space would collapse into the space.
    let base_advance = base.font_size;

    let raise = base.baseline_offset;
    let group_ascent = ascent + raise.max(0.0);
    let group_descent = descent + (-raise).max(0.0);
    let baseline = group_ascent - raise;
    let glyph_run = |x: f64, y: f64, shaped: &ShapedText, text: &str, field_base: bool| {
        PositionedElement::Text(GlyphRun {
            origin: Point { x, y },
            font_id: base.font_id,
            font_size: base.font_size,
            glyph_ids: shaped.glyph_ids.clone(),
            advances: shaped.advances.clone(),
            text: text.to_owned(),
            source,
            color: base.color,
            bold: base.bold,
            italic: base.italic,
            field_kind: if field_base { base.field_kind } else { None },
            field_source: if field_base {
                base.occurrence.map(|owner| owner.source)
            } else {
                None
            },
            note: None,
            note_reference_source: None,
            tab_aligned: None,
        })
    };

    if combine {
        let cell = base_advance;
        let (open, close) = combine_bracket_pair(layout.combine_brackets.as_deref());
        let open_shaped = match open.is_empty() {
            true => None,
            false => {
                Some(fm.shape_text_with_kerning(base.font_id, open, base.font_size, base.kern)?)
            }
        };
        let close_shaped = match close.is_empty() {
            true => None,
            false => {
                Some(fm.shape_text_with_kerning(base.font_id, close, base.font_size, base.kern)?)
            }
        };
        let open_width = open_shaped.as_ref().map_or(0.0, |shaped| shaped.width);
        let close_width = close_shaped.as_ref().map_or(0.0, |shaped| shaped.width);
        let width = open_width + cell + close_width;
        let mut children = Vec::new();
        let mut policies = PaintPolicies::default();
        if let Some(highlight) = base.highlight {
            children.push(PositionedElement::FilledRect {
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    width,
                    height: group_ascent + group_descent,
                },
                color: highlight,
            });
        }
        if let Some(ref open_shaped) = open_shaped {
            children.push(glyph_run(0.0, baseline, open_shaped, open, false));
        }
        let mut base_children = PaintedElements::default();
        let base_glyph = glyph_run(0.0, baseline, &shaped, text, true);
        if base.occurrence.is_some() {
            base_children.push_field(base_glyph, annotation_field_owner(base))?;
        } else {
            base_children.push_non_field(base_glyph)?;
        }
        policies.append(
            &mut children,
            base_children.wrap(PositionedElement::Group(GroupElement {
                transform: Transform {
                    a: cell / shaped.width,
                    e: open_width,
                    ..Transform::IDENTITY
                },
                clip: None,
                opacity: 1.0,
                effects: Vec::new(),
                children: Vec::new(),
            }))?,
        )?;
        if let Some(ref close_shaped) = close_shaped {
            children.push(glyph_run(
                open_width + cell,
                baseline,
                close_shaped,
                close,
                false,
            ));
        }
        push_annotation_decorations(&mut children, base, baseline, width, ascent, descent);
        register_inline_annotation(
            styles,
            inline_items.len(),
            recipe.clone(),
            policies,
            WordAnnotationRootKind::Group,
        );
        inline_items.push(InlineItem::Group {
            width,
            height: group_ascent + group_descent,
            baseline: Some(group_ascent),
            math: false,
            group: GroupElement {
                transform: Transform::IDENTITY,
                clip: None,
                opacity: 1.0,
                effects: Vec::new(),
                children,
            },
        });
        fm.record_font_contribution(base.font_id, base.font_family, text)?;
        if open_shaped.is_some() {
            fm.record_font_contribution(base.font_id, base.font_family, open)?;
        }
        if close_shaped.is_some() {
            fm.record_font_contribution(base.font_id, base.font_family, close)?;
        }
        return Ok(true);
    }

    // `w:vert`. The run is laid out horizontally and rotated 90 degrees, so
    // its own line height becomes the advance it takes along the line and its
    // text length becomes the height it needs.
    let thickness = group_ascent + group_descent;
    if thickness <= 0.0 {
        return Ok(false);
    }
    // A compressed run is narrowed to one base-character advance, and never
    // widened, because compression may only take space away.
    let compressed = layout.vert_compress == Some(true) && base_advance < thickness;
    let width = if compressed { base_advance } else { thickness };
    let mut children = Vec::new();
    let mut policies = PaintPolicies::default();
    if let Some(highlight) = base.highlight {
        children.push(PositionedElement::FilledRect {
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width: shaped.width,
                height: thickness,
            },
            color: highlight,
        });
    }
    policies.emit(
        &mut children,
        glyph_run(0.0, baseline, &shaped, text, true),
        annotation_field_owner(base),
    )?;
    push_annotation_decorations(&mut children, base, baseline, shaped.width, ascent, descent);
    // Rotate about the group origin, then bring the rotated box back into
    // positive x, then narrow it when `w:vertCompress` asked for that.
    let mut transform = Transform::rotate_about(90.0, 0.0, 0.0).then(Transform {
        e: thickness,
        ..Transform::IDENTITY
    });
    if compressed {
        transform = transform.then(Transform {
            a: width / thickness,
            ..Transform::IDENTITY
        });
    }
    register_inline_annotation(
        styles,
        inline_items.len(),
        recipe,
        policies,
        WordAnnotationRootKind::Group,
    );
    inline_items.push(InlineItem::Group {
        width,
        height: shaped.width,
        baseline: None,
        math: false,
        group: GroupElement {
            transform,
            clip: None,
            opacity: 1.0,
            effects: Vec::new(),
            children,
        },
    });
    fm.record_font_contribution(base.font_id, base.font_family, text)?;
    Ok(true)
}

/// Record a diagnostic once, matching the media registry's dedupe by message.
pub(crate) fn push_unique_diagnostic(diagnostics: &mut Vec<Diagnostic>, message: String) {
    if !diagnostics.iter().any(|entry| entry.message == message) {
        diagnostics.push(Diagnostic { message });
    }
}

/// Draw the decorations a `TextSegment` would have carried, in group-local
/// coordinates against a baseline at `baseline`.
pub(super) fn push_annotation_decorations(
    children: &mut Vec<PositionedElement>,
    base: &AnnotationBase<'_>,
    baseline: f64,
    width: f64,
    ascent: f64,
    descent: f64,
) {
    let line_at = |y: f64, thickness: f64, children: &mut Vec<PositionedElement>| {
        children.push(PositionedElement::Line {
            start: Point { x: 0.0, y },
            end: Point { x: width, y },
            width: thickness,
            color: base.color,
            dash_pattern: None,
        });
    };
    if let Some(style) = base.underline {
        let underline_y = baseline + descent * 0.3;
        let (thickness, second) = convert::underline_strokes(style, base.font_size);
        line_at(underline_y, thickness, children);
        if let Some(offset) = second {
            line_at(underline_y + offset, thickness, children);
        }
    }
    let thickness = base.font_size / 24.0;
    let strike_y = baseline - ascent * 0.3;
    if base.strike {
        line_at(strike_y, thickness, children);
    }
    if base.dstrike {
        let gap = thickness * 2.0;
        line_at(strike_y - gap / 2.0, thickness, children);
        line_at(strike_y + gap / 2.0, thickness, children);
    }
}

/// Project one emphasis-marked text run into inline items.
///
/// Reports `false` when the mark cannot be drawn, which leaves the caller on
/// the ordinary text path so the base text is painted unchanged. An
/// undrawable mark codepoint records a diagnostic and paints nothing, which
/// is the policy the uncovered-character path already follows.
pub(super) fn push_emphasis_marked_text(
    inline_items: &mut Vec<InlineItem>,
    styles: &mut HashMap<usize, WordMultilingualStyle>,
    fm: &mut FontManager,
    diagnostics: &mut Vec<Diagnostic>,
    mark: &ST_Em,
    text: &str,
    base: &AnnotationBase<'_>,
) -> Result<bool> {
    let recipe = annotation_recipe(WordAnnotationKind::Emphasis(mark.clone()), base, None)?;
    let Some((glyph, above)) = emphasis_mark_glyph(mark) else {
        return Ok(false);
    };
    let mark_text = glyph.to_string();
    let mark_size = base.font_size * EMPHASIS_MARK_SCALE;
    let mark_font =
        fm.resolve_font_for_text(base.font_family, base.bold, base.italic, &mark_text)?;
    let mark_shaped = fm.shape_text_with_kerning(mark_font, &mark_text, mark_size, base.kern)?;
    if mark_shaped.glyph_ids.is_empty() || mark_shaped.glyph_ids.contains(&0) {
        push_unique_diagnostic(
            diagnostics,
            format!(
                "emphasis mark {} has no glyph in the resolved font and is not painted",
                mark.as_str()
            ),
        );
        return Ok(false);
    }
    let mark_metrics = fm.metrics(mark_font, mark_size)?;
    let metrics = requested_metrics(fm, base)?;
    let ascent = metrics.ascent.max(0.0);
    let descent = metrics.descent.max(0.0);
    let mark_ascent = mark_metrics.ascent.max(0.0);
    let mark_descent = mark_metrics.descent.max(0.0);
    let mark_height = mark_ascent + mark_descent;

    let raise = base.baseline_offset;
    let group_ascent = if above { ascent + mark_height } else { ascent } + raise.max(0.0);
    let group_descent = if above {
        descent
    } else {
        descent + mark_height
    } + (-raise).max(0.0);
    let baseline = group_ascent - raise;
    let mark_baseline = if above {
        baseline - ascent - mark_descent
    } else {
        baseline + descent + mark_ascent
    };

    for chunk in emphasis_chunks(text) {
        let mut shaped =
            fm.shape_text_with_kerning(base.font_id, chunk, base.font_size, base.kern)?;
        if base.spacing != 0.0 {
            for advance in &mut shaped.advances {
                *advance += base.spacing;
            }
            shaped.width += base.spacing * shaped.advances.len() as f64;
        }
        let characters = chunk.chars().collect::<Vec<_>>();
        let mut children = Vec::new();
        let mut policies = PaintPolicies::default();
        if let Some(highlight) = base.highlight {
            children.push(PositionedElement::FilledRect {
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: shaped.width,
                    height: group_ascent + group_descent,
                },
                color: highlight,
            });
        }
        policies.emit(
            &mut children,
            PositionedElement::Text(GlyphRun {
                origin: Point {
                    x: 0.0,
                    y: baseline,
                },
                font_id: base.font_id,
                font_size: base.font_size,
                glyph_ids: shaped.glyph_ids.clone(),
                advances: shaped.advances.clone(),
                text: chunk.to_owned(),
                source: None,
                color: base.color,
                bold: base.bold,
                italic: base.italic,
                field_kind: base.field_kind,
                field_source: base.occurrence.map(|owner| owner.source),
                note: None,
                note_reference_source: None,
                tab_aligned: None,
            }),
            annotation_field_owner(base),
        )?;
        push_annotation_decorations(&mut children, base, baseline, shaped.width, ascent, descent);

        // One mark per base character. Glyphs and characters correspond one
        // to one for the scripts `w:em` applies to, and when shaping has
        // merged them the marks fall back to an even share of the chunk so
        // the count still matches the characters.
        let advances = if shaped.advances.len() == characters.len() {
            shaped.advances.clone()
        } else if characters.is_empty() {
            Vec::new()
        } else {
            vec![shaped.width / characters.len() as f64; characters.len()]
        };
        let mut x = 0.0;
        for (character, advance) in characters.iter().zip(&advances) {
            if !character.is_whitespace() {
                children.push(PositionedElement::Text(GlyphRun {
                    origin: Point {
                        x: x + (advance - mark_shaped.width) / 2.0,
                        y: mark_baseline,
                    },
                    font_id: mark_font,
                    font_size: mark_size,
                    glyph_ids: mark_shaped.glyph_ids.clone(),
                    advances: mark_shaped.advances.clone(),
                    text: mark_text.clone(),
                    source: None,
                    color: base.color,
                    bold: false,
                    italic: false,
                    field_kind: None,
                    field_source: None,
                    note: None,
                    note_reference_source: None,
                    tab_aligned: None,
                }));
            }
            x += advance;
        }

        fm.record_font_contribution(base.font_id, base.font_family, chunk)?;
        if characters
            .iter()
            .any(|character| !character.is_whitespace())
        {
            fm.record_font_contribution(mark_font, base.font_family, &mark_text)?;
        }
        register_inline_annotation(
            styles,
            inline_items.len(),
            recipe.clone(),
            policies,
            WordAnnotationRootKind::Group,
        );
        inline_items.push(InlineItem::Group {
            width: shaped.width,
            height: group_ascent + group_descent,
            baseline: Some(group_ascent),
            math: false,
            group: GroupElement {
                transform: Transform::IDENTITY,
                clip: None,
                opacity: 1.0,
                effects: Vec::new(),
                children,
            },
        });
    }
    Ok(true)
}

/// The metrics a base run's annotation group measures on: the face of its
/// requested family, not a face picked for glyphs that family lacks, as body
/// text measures.
fn requested_metrics(
    fm: &mut FontManager,
    base: &AnnotationBase<'_>,
) -> Result<oxoffice_layout::FontMetrics> {
    let face = fm.resolve_font_for_metrics(base.font_family, base.bold, base.italic)?;
    fm.metrics(face, base.font_size)
}

/// Shape a sequence of runs onto one baseline.
///
/// `size_override` is the size `w:hps` and `w:hpsBaseText` fix for the whole
/// line, which is why it wins over the run's own `w:sz`.
fn measure_annotation_line(
    runs: &[CT_R],
    context: &AnnotationContext<'_>,
    fm: &mut FontManager,
    size_override: Option<f64>,
) -> Result<AnnotationLine> {
    let mut line = AnnotationLine {
        width: 0.0,
        ascent: 0.0,
        descent: 0.0,
        primary_font_size: size_override.unwrap_or(0.0),
        runs: Vec::new(),
    };
    for run in runs {
        let run_style_id = run.properties.as_ref().and_then(|p| p.style_id.as_deref());
        let mut effective_rpr = style_resolver::resolve_run_properties(
            context.para_style_id,
            run_style_id,
            context.styles,
            context.table_run_properties,
        );
        if let Some(ref direct_rpr) = run.properties {
            effective_rpr.merge_from(direct_rpr);
        }
        if effective_rpr.vanish == Some(true) {
            continue;
        }
        let text = run.text();
        if text.is_empty() {
            continue;
        }
        let font_size =
            size_override.unwrap_or_else(|| effective_rpr.sz.map(|hp| hp.to_pt()).unwrap_or(11.0));
        let bold = effective_rpr.bold.unwrap_or(false);
        let italic = effective_rpr.italic.unwrap_or(false);
        let family = resolve_font_family(
            &effective_rpr,
            context.input.theme.as_ref(),
            word_font_slot_for_text(&text, effective_rpr.font_hint.as_deref()),
        );
        let font_id = fm.resolve_font_for_text(family.as_deref(), bold, italic, &text)?;
        let shaped = fm.shape_text_with_kerning(
            font_id,
            &text,
            font_size,
            style_resolver::word_kerning_enabled(&effective_rpr),
        )?;
        // The face of the requested family measures the line, as in body text.
        let measure_font = fm.resolve_font_for_metrics(family.as_deref(), bold, italic)?;
        let metrics = fm.metrics(measure_font, font_size)?;
        fm.record_font_contribution(font_id, family.as_deref(), &text)?;
        if line.runs.is_empty() {
            line.primary_font_size = font_size;
        }
        line.ascent = line.ascent.max(metrics.ascent.max(0.0));
        line.descent = line.descent.max(metrics.descent.max(0.0));
        line.runs.push(GlyphRun {
            origin: Point {
                x: line.width,
                y: 0.0,
            },
            font_id,
            font_size,
            glyph_ids: shaped.glyph_ids,
            advances: shaped.advances,
            text,
            source: None,
            color: resolve_run_color(
                &effective_rpr,
                context.input.theme.as_ref(),
                context.automatic_color,
            ),
            bold,
            italic,
            field_kind: None,
            field_source: None,
            note: None,
            note_reference_source: None,
            tab_aligned: None,
        });
        line.width += shaped.width;
    }
    Ok(line)
}

/// Distribute a phonetic line over the base width per `w:rubyAlign`.
///
/// The two distribute modes widen the gaps between phonetic glyphs, which is
/// what the mode names describe, so they act on the glyph advances rather
/// than on the line origin the other modes move.
fn distribute_ruby_line(line: &mut AnnotationLine, align: Option<&ST_RubyAlign>, width: f64) {
    let slack = width - line.width;
    if slack <= 0.0 {
        return;
    }
    let glyphs = line
        .runs
        .iter()
        .map(|run| run.advances.len())
        .sum::<usize>();
    let (start, gap) = match align.unwrap_or(&ST_RubyAlign::Center) {
        ST_RubyAlign::Left => (0.0, 0.0),
        ST_RubyAlign::Right | ST_RubyAlign::RightVertical => (slack, 0.0),
        ST_RubyAlign::DistributeLetter if glyphs > 1 => (0.0, slack / (glyphs - 1) as f64),
        ST_RubyAlign::DistributeSpace if glyphs > 0 => {
            let gap = slack / (glyphs + 1) as f64;
            (gap, gap)
        }
        // Centre is the fallback for the remaining tokens, including a
        // producer value this renderer has no distribution rule for.
        _ => (slack / 2.0, 0.0),
    };
    let mut x = start;
    let mut remaining = glyphs;
    for run in &mut line.runs {
        run.origin.x = x;
        for advance in &mut run.advances {
            remaining -= 1;
            if remaining > 0 {
                *advance += gap;
            }
            x += *advance;
        }
    }
    line.width = width;
}

/// Project one ruby annotation into an inline item.
///
/// The base line and the phonetic line are measured independently, the
/// phonetic line is placed above the base at `w:hpsRaise` in the `w:hps`
/// size, and the annotation's ascent carries both, which is what makes a
/// ruby-bearing line taller for the paginator.
pub(super) fn ruby_inline_item(
    ruby: &CT_Ruby,
    base_runs: &[CT_R],
    context: &AnnotationContext<'_>,
    fm: &mut FontManager,
) -> Result<Option<InlineItem>> {
    let properties = ruby.properties.as_ref();
    let base_override = properties
        .and_then(|properties| properties.hps_base_text)
        .map(|size| size.to_pt());
    let mut base = measure_annotation_line(base_runs, context, fm, base_override)?;
    if base.runs.is_empty() {
        return Ok(None);
    }
    let phonetic_size = properties
        .and_then(|properties| properties.hps)
        .map(|size| size.to_pt())
        .unwrap_or(base.primary_font_size / 2.0);
    let mut phonetic = measure_annotation_line(&ruby.ruby_text, context, fm, Some(phonetic_size))?;

    let width = base.width.max(phonetic.width);
    let raise = properties
        .and_then(|properties| properties.hps_raise)
        .map(|size| size.to_pt())
        .unwrap_or(base.ascent + phonetic.descent);
    let group_ascent = base.ascent.max(raise + phonetic.ascent);
    let group_descent = base.descent;

    distribute_ruby_line(&mut base, Some(&ST_RubyAlign::Center), width);
    distribute_ruby_line(
        &mut phonetic,
        properties.and_then(|properties| properties.align.as_ref()),
        width,
    );

    let mut children = Vec::new();
    for mut run in base.runs {
        run.origin.y = group_ascent;
        children.push(PositionedElement::Text(run));
    }
    for mut run in phonetic.runs {
        run.origin.y = group_ascent - raise;
        children.push(PositionedElement::Text(run));
    }

    Ok(Some(InlineItem::Group {
        width,
        height: group_ascent + group_descent,
        baseline: Some(group_ascent),
        math: false,
        group: GroupElement {
            transform: Transform::IDENTITY,
            clip: None,
            opacity: 1.0,
            effects: Vec::new(),
            children,
        },
    }))
}

/// Resolve the effective color for a run, considering theme colors.
///
/// Priority: solid text fill > resolved theme color > literal fallback > `automatic`, the
/// automatic colour that [`automatic_text_color`] gives for the fill behind
/// the text.
pub(super) fn resolve_run_color(
    rpr: &oxoffice_docx_oxml::properties::CT_RPr,
    theme: Option<&oxoffice_docx_oxml::theme::Theme>,
    automatic: Color,
) -> Color {
    if let Some((hex, alpha)) = rpr.solid_text_fill() {
        let mut color = Color::from_hex(&hex);
        color.a = alpha;
        return color;
    }

    // If theme color is specified, resolve it from the theme
    if let Some(ref theme_name) = rpr.color_theme
        && let Some(theme) = theme
        && let Some(hex) = theme.colors.get(theme_name)
    {
        // `w:themeTint` and `w:themeShade` are Word's 0-255 byte convention,
        // which is exactly what `oxoffice_docx_oxml::theme::apply_tint_shade` takes.
        // Its arithmetic is Word's, deliberately not the spec-correct
        // DrawingML one, and it is called unchanged. See "Things that are
        // deliberately wrong" in docs/ARCHITECTURE.md.
        if rpr.color_theme_tint.is_some() || rpr.color_theme_shade.is_some() {
            return Color::from_hex(&oxoffice_docx_oxml::theme::apply_tint_shade(
                hex,
                rpr.color_theme_tint,
                rpr.color_theme_shade,
            ));
        }
        return Color::from_hex(hex);
    }

    // Fall back to literal color value
    rpr.color
        .as_ref()
        .filter(|c| c.as_str() != "auto")
        .map(|c| Color::from_hex(c))
        .unwrap_or(automatic)
}

/// The colour a `w:shd` paints: its fill under a clear pattern, its
/// foreground under a solid one, the two mixed under a percentage pattern,
/// and nothing for `nil` or an automatic fill. Under a pattern Word paints an
/// automatic foreground black and an automatic fill white. Other patterns
/// (stripes, crosses) are painted as their fill.
pub(crate) fn shading_color(shading: &CT_Shd) -> Option<Color> {
    let hex = |value: &Option<String>| {
        value
            .as_deref()
            .filter(|value| *value != "auto")
            .map(Color::from_hex)
    };
    let percent = match shading.val.as_str() {
        "nil" => return None,
        "solid" => 100.0,
        // Four patterns stand for a half percent.
        "pct12" => 12.5,
        "pct37" => 37.5,
        "pct62" => 62.5,
        "pct87" => 87.5,
        pattern => pattern
            .strip_prefix("pct")
            .and_then(|percent| percent.parse::<f64>().ok())
            .unwrap_or(0.0),
    };
    if percent == 0.0 {
        return hex(&shading.fill);
    }
    let foreground = hex(&shading.color).unwrap_or(Color::BLACK);
    let background = hex(&shading.fill).unwrap_or(Color::WHITE);
    let mix = |front: f64, back: f64| (front * percent + back * (100.0 - percent)) / 100.0;
    Some(Color {
        r: mix(foreground.r, background.r),
        g: mix(foreground.g, background.g),
        b: mix(foreground.b, background.b),
        a: 1.0,
    })
}

/// The fill drawn behind a run's text: its highlight, which Word draws over
/// its shading, or else its shading.
pub(super) fn run_fill(rpr: &CT_RPr) -> Option<Color> {
    rpr.highlight
        .and_then(highlight_to_color)
        .or_else(|| rpr.shading.as_deref().and_then(shading_color))
}

/// The automatic text colour on `fill`: white on a dark fill, black on a
/// light one or on none. Word draws automatic text in whichever contrasts
/// with the shading behind it. The coefficients and the luminance floor of
/// 156 in 255 are the donor's chosen policy, not a pinned LibreOffice source.
pub(super) fn automatic_text_color(fill: Option<Color>) -> Color {
    let Some(fill) = fill else {
        return Color::BLACK;
    };
    let byte = |channel: f64| (channel.clamp(0.0, 1.0) * 255.0).round();
    let luminance = (76.0 * byte(fill.r) + 151.0 * byte(fill.g) + 29.0 * byte(fill.b)) / 256.0;
    if luminance.floor() <= 156.0 {
        Color::WHITE
    } else {
        Color::BLACK
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WordLanguageSlot {
    Direct,
    EastAsia,
    Bidi,
}

pub(super) fn word_language_slot(character: char) -> Option<WordLanguageSlot> {
    match character as u32 {
        0x0590..=0x08ff | 0xfb1d..=0xfdff | 0xfe70..=0xfeff => Some(WordLanguageSlot::Bidi),
        // A character East Asian to one table and absent from this one would
        // be attached to whichever slot preceded it, and at the start of a
        // run that is the direct slot, so it would silently take
        // `w:lang/@w:val` where Word takes `@w:eastAsia`.
        codepoint if is_east_asian(codepoint) => Some(WordLanguageSlot::EastAsia),
        // The Latin letters, including the IPA extensions, which are letters
        // in their own right.
        //
        // This stops short of the spacing modifier letters and modifier
        // symbols at 0x02b0..=0x02ff, which `word_font_slot` does claim for
        // `w:hAnsi`. The two tables have different residual cases, so
        // mirroring a boundary between them is the wrong operation. `None`
        // here means inherit, and `word_language_ranges` leaves an unclaimed
        // character inside the range it fell in, which is what a modifier
        // should do. Claiming them would split a Bopomofo syllable from its
        // tone mark, since `U+02C7`, `U+02CA`, `U+02CB` and `U+02D9` all live
        // there, and hand the mark `w:lang/@w:val` where Word keeps the
        // syllable's `@w:eastAsia`.
        0x0041..=0x02af | 0x0900..=0x097f | 0x0e00..=0x0e7f | 0x1e00..=0x1eff => {
            Some(WordLanguageSlot::Direct)
        }
        _ => None,
    }
}

//! Style resolution: cascade styles and generate numbering markers.
//!
//! Ports the logic from `crates/rdocx/src/style.rs` since oxoffice-docx-layout
//! depends on oxoffice-docx-oxml directly (not rdocx).

use std::collections::{HashMap, HashSet};

use oxoffice_docx_oxml::borders::CT_Tabs;
use oxoffice_docx_oxml::numbering::{CT_Lvl, CT_Numbering, ST_LvlSuffix, ST_NumberFormat};
use oxoffice_docx_oxml::properties::{CT_PPr, CT_RPr};
use oxoffice_docx_oxml::shared::ST_TabJc;
use oxoffice_docx_oxml::styles::{CT_Style, CT_Styles, StyleType};
use oxoffice_layout::{NoteRef, SourceNodeId};

/// A fully resolved paragraph with merged properties and numbering info.
#[derive(Debug, Clone)]
pub struct ResolvedParagraph {
    /// Merged paragraph properties (style chain + direct formatting).
    pub ppr: CT_PPr,
    /// Resolved runs with merged run properties.
    pub runs: Vec<ResolvedRun>,
    /// Numbering marker info (if paragraph is part of a list).
    pub numbering: Option<ResolvedNumbering>,
}

/// A run with fully resolved properties.
#[derive(Debug, Clone)]
pub struct ResolvedRun {
    /// Merged run properties.
    pub rpr: CT_RPr,
    /// Run content items.
    pub content: Vec<oxoffice_docx_oxml::text::RunContent>,
}

/// Resolved numbering marker for a list paragraph.
#[derive(Debug, Clone)]
pub struct ResolvedNumbering {
    /// The text of the marker (e.g., "1.", "a)", bullet char).
    pub marker_text: String,
    /// The current level's formatted counter without surrounding level text.
    pub number_current: String,
    /// The current level's complete number expression without trailing stops.
    #[doc(hidden)]
    pub number_level: String,
    /// The current level's number expression with literal text removed.
    #[doc(hidden)]
    pub number_level_without_text: String,
    /// Whether the current level expression embeds an ancestor placeholder.
    #[doc(hidden)]
    pub number_level_has_ancestor: bool,
    /// The complete visible number without trailing full stops.
    pub number_full: String,
    /// The complete contextual number with literal text removed.
    #[doc(hidden)]
    pub number_full_without_text: String,
    /// Formatted counters from the root level through the current level.
    #[doc(hidden)]
    pub number_context: Vec<String>,
    /// Contextual number suffixes beginning at each level, using source delimiters.
    #[doc(hidden)]
    pub number_suffixes_without_text: Vec<String>,
    /// Concrete numbering instance that owns this counter sequence.
    #[doc(hidden)]
    pub num_id: u32,
    /// Abstract numbering definition that owns this counter sequence.
    #[doc(hidden)]
    pub definition_id: u32,
    /// Whether any level of this concrete instance carries a start override.
    #[doc(hidden)]
    pub has_start_override: bool,
    /// Run properties for the marker.
    pub marker_rpr: CT_RPr,
    /// Item that follows the marker before paragraph content begins.
    pub suffix: ST_LvlSuffix,
}

impl ResolvedNumbering {
    /// Format REF paragraph-number context and its current-level delimiter.
    /// Position suffixes and hyperlinks remain the caller's responsibility.
    #[doc(hidden)]
    pub fn numbered_reference_text(
        &self,
        instruction: &oxoffice_docx_oxml::text::FieldInstruction,
        source: Option<&Self>,
    ) -> std::result::Result<String, String> {
        use oxoffice_docx_oxml::text::FieldArgument;
        if !matches!(instruction.name.as_str(), "REF" | "STYLEREF") {
            return Err("paragraph-number reference formatting requires REF or STYLEREF".into());
        }
        // STYLEREF without a numbered caller compares the target with itself,
        // so relative context is its current level. REF still keeps full context.
        let source = source.or_else(|| (instruction.name == "STYLEREF").then_some(self));
        let has = |name| {
            instruction
                .switches
                .iter()
                .any(|switch| switch.name == name)
        };
        let omit_text = has("t");
        let value = if has("w") {
            if omit_text {
                self.number_full_without_text.clone()
            } else {
                self.number_full.clone()
            }
        } else if has("r") {
            self.relative_to(source, omit_text)
        } else if has("n") {
            if omit_text {
                self.number_level_without_text.clone()
            } else {
                self.number_level.clone()
            }
        } else {
            return Err("REF paragraph-number formatting requires n, r or w".into());
        };
        if instruction.name == "STYLEREF" {
            return Ok(value);
        }
        let mut delimiters = instruction
            .switches
            .iter()
            .filter(|switch| switch.name == "d");
        let Some(delimiter) = delimiters.next() else {
            return Ok(value);
        };
        if delimiters.next().is_some() {
            return Err("REF delimiter switches are ambiguous".into());
        }
        let Some(FieldArgument::Text(delimiter)) = &delimiter.argument else {
            return Err("REF delimiter requires a text operand".into());
        };
        let current = if omit_text {
            &self.number_level_without_text
        } else {
            &self.number_level
        };
        if current.is_empty() {
            return if value.is_empty() {
                Ok(value)
            } else {
                Err("REF delimiter lacks a qualified current-level boundary".into())
            };
        }
        let prefix = value
            .strip_suffix(current)
            .ok_or_else(|| "REF delimiter lacks a qualified current-level boundary".to_owned())?;
        if prefix.is_empty() {
            return Ok(value);
        }
        // Actual Word controls preserve the ancestor stops and insert only at
        // this current-level boundary. An explicit empty operand inserts space.
        Ok(format!(
            "{prefix}{}{current}",
            if delimiter.is_empty() { " " } else { delimiter }
        ))
    }

    /// Format this paragraph number relative to a source paragraph.
    #[doc(hidden)]
    pub fn relative_to(&self, source: Option<&Self>, omit_text: bool) -> String {
        let shares_context = source.is_some_and(|source| {
            source.num_id == self.num_id
                || (source.definition_id == self.definition_id
                    && !source.has_start_override
                    && !self.has_start_override)
        });
        let mut start = source
            .filter(|_| shares_context)
            .map(|source| {
                self.number_context
                    .iter()
                    .zip(&source.number_context)
                    .take_while(|(target, source)| target == source)
                    .count()
                    .min(self.number_context.len().saturating_sub(1))
            })
            .unwrap_or(0);
        if start < self.number_context.len().saturating_sub(1) && !self.number_level_has_ancestor {
            start = 0;
        }
        let suffix = if self.number_level_has_ancestor
            && start == self.number_context.len().saturating_sub(1)
        {
            self.number_level_without_text.clone()
        } else {
            self.number_suffixes_without_text
                .get(start)
                .cloned()
                .unwrap_or_else(|| self.number_full_without_text.clone())
        };
        if omit_text {
            return suffix;
        }
        match suffix.strip_suffix(&self.number_level_without_text) {
            Some(prefix) => format!("{prefix}{}", self.number_level),
            None => suffix,
        }
    }
}

/// Tracks numbering counters across paragraphs.
///
/// `Clone` exists so a note laid out at more than one section width consumes
/// its list numbers once rather than once per width.
#[derive(Clone)]
pub struct NumberingState {
    /// (abstractNumId, ilvl) owns the visible sequence after native resolution.
    counters: HashMap<(u32, u32), u32>,
    /// Concrete IDs remain public source identities, not sequence identities.
    definition_by_instance: HashMap<u32, u32>,
    consumed_overrides: HashSet<(u32, u32)>,
    pending_sections: HashSet<u32>,
    resolved_by_source: HashMap<SourceNodeId, ResolvedNumbering>,
    pub(crate) navigation: HashMap<SourceNodeId, crate::ParagraphNavigation>,
    bookmark_sources: HashMap<String, SourceNodeId>,
    resolved_by_bookmark: HashMap<String, (SourceNodeId, ResolvedNumbering)>,
    main_story_sources: HashSet<SourceNodeId>,
    note_labels: HashMap<NoteRef, String>,
    // Set only while shaping an actual note story. Never a body reference.
    pub(crate) note_body_context: Option<NoteRef>,
}

impl Default for NumberingState {
    fn default() -> Self {
        Self::new()
    }
}

impl NumberingState {
    pub fn new() -> Self {
        NumberingState {
            counters: HashMap::new(),
            definition_by_instance: HashMap::new(),
            consumed_overrides: HashSet::new(),
            pending_sections: HashSet::new(),
            resolved_by_source: HashMap::new(),
            navigation: HashMap::new(),
            bookmark_sources: HashMap::new(),
            resolved_by_bookmark: HashMap::new(),
            main_story_sources: HashSet::new(),
            note_labels: HashMap::new(),
            note_body_context: None,
        }
    }

    /// Advance the counter for the given numId/ilvl and return the new value.
    pub fn advance(&mut self, num_id: u32, ilvl: u32, start: u32) -> u32 {
        let owner = self
            .definition_by_instance
            .get(&num_id)
            .copied()
            .unwrap_or(num_id);
        let key = (owner, ilvl);
        match self.counters.entry(key) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(start);
                start
            }
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let value = entry.get().saturating_add(1);
                entry.insert(value);
                value
            }
        }
    }

    /// Get the current count for a level (without advancing).
    pub fn current(&self, num_id: u32, ilvl: u32) -> u32 {
        let owner = self
            .definition_by_instance
            .get(&num_id)
            .copied()
            .unwrap_or(num_id);
        self.counters.get(&(owner, ilvl)).copied().unwrap_or(0)
    }

    fn restart_deeper_levels(
        &mut self,
        num_id: u32,
        definition_id: u32,
        current_level: u32,
        numbering: &CT_Numbering,
        rearm_current_instance: bool,
    ) {
        for deeper in (current_level + 1)..=8 {
            let restart = resolved_numbering_level(num_id, deeper, numbering)
                .and_then(|(base, _, _)| base.restart);
            let should_restart = match restart {
                None => true,
                Some(0) => false,
                Some(owner) => current_level < owner,
            };
            if should_restart {
                self.counters.remove(&(definition_id, deeper));
                if rearm_current_instance {
                    self.consumed_overrides.remove(&(num_id, deeper));
                }
            }
        }
    }

    /// Defer opted-in definition resets until their next actual numbered item.
    pub fn restart_after_section_break(&mut self, numbering: &CT_Numbering) {
        for instance in &numbering.nums {
            if numbering.restarts_after_section_break(instance.num_id)
                && self
                    .counters
                    .keys()
                    .any(|(owner, _)| *owner == instance.abstract_num_id)
            {
                self.pending_sections.insert(instance.abstract_num_id);
            }
        }
    }

    fn advance_numbering(
        &mut self,
        num_id: u32,
        definition_id: u32,
        level: u32,
        base_start: u32,
        override_start: Option<u32>,
        numbering: &CT_Numbering,
    ) -> u32 {
        self.definition_by_instance.insert(num_id, definition_id);
        let first_after_section = self.pending_sections.remove(&definition_id);
        if first_after_section {
            let root_start = resolved_numbering_level(num_id, 0, numbering)
                .and_then(|(base, _, _)| base.start)
                .unwrap_or(1);
            self.counters.insert((definition_id, 0), root_start);
            // A section event preserves concrete override-use state. A child
            // with lvlRestart=0 also retains its carried shared count.
            self.restart_deeper_levels(num_id, definition_id, 0, numbering, false);
            if level == 0 {
                return root_start;
            }
        }
        self.restart_deeper_levels(
            num_id,
            definition_id,
            level,
            numbering,
            !first_after_section,
        );
        if !first_after_section
            && let Some(start) = override_start
            && self.consumed_overrides.insert((num_id, level))
        {
            self.counters.insert((definition_id, level), start);
            return start;
        }
        self.advance(num_id, level, base_start)
    }

    pub(crate) fn record_navigation(
        &mut self,
        source: SourceNodeId,
        facts: crate::ParagraphNavigation,
    ) {
        self.navigation.insert(source, facts);
    }

    pub(crate) fn remove_navigation(
        &mut self,
        source: SourceNodeId,
    ) -> Option<crate::ParagraphNavigation> {
        self.navigation.remove(&source)
    }

    pub(crate) fn take_navigation(&mut self) -> HashMap<SourceNodeId, crate::ParagraphNavigation> {
        std::mem::take(&mut self.navigation)
    }

    pub(crate) fn resolved_for_source(&self, source: SourceNodeId) -> Option<&ResolvedNumbering> {
        self.resolved_by_source.get(&source)
    }

    pub(crate) fn record(&mut self, source: SourceNodeId, numbering: &ResolvedNumbering) {
        self.resolved_by_source.insert(source, numbering.clone());
    }

    pub(crate) fn record_bookmark(
        &mut self,
        name: &str,
        source: SourceNodeId,
        numbering: &ResolvedNumbering,
    ) {
        self.resolved_by_bookmark
            .insert(name.to_owned(), (source, numbering.clone()));
    }

    pub(crate) fn record_bookmark_source(&mut self, name: &str, source: SourceNodeId) {
        self.bookmark_sources.insert(name.to_owned(), source);
    }

    pub(crate) fn bookmark_sources(&self) -> impl Iterator<Item = (&str, SourceNodeId)> {
        self.bookmark_sources
            .iter()
            .map(|(name, node)| (name.as_str(), *node))
    }

    pub(crate) fn bookmark_source(&self, name: &str) -> Option<SourceNodeId> {
        self.bookmark_sources.get(name).copied()
    }

    pub(crate) fn bookmark(&self, name: &str) -> Option<(SourceNodeId, &ResolvedNumbering)> {
        self.resolved_by_bookmark
            .get(name)
            .map(|(source, numbering)| (*source, numbering))
    }

    pub(crate) fn source(&self, source: SourceNodeId) -> Option<&ResolvedNumbering> {
        self.resolved_by_source.get(&source)
    }

    pub(crate) fn record_main_story_source(&mut self, source: SourceNodeId) {
        self.main_story_sources.insert(source);
    }

    pub(crate) fn is_main_story_source(&self, source: SourceNodeId) -> bool {
        self.main_story_sources.contains(&source)
    }

    pub(crate) fn references_only(&self) -> Self {
        Self {
            counters: HashMap::new(),
            definition_by_instance: HashMap::new(),
            consumed_overrides: HashSet::new(),
            pending_sections: HashSet::new(),
            resolved_by_source: HashMap::new(),
            navigation: self
                .navigation
                .iter()
                .filter(|(_, facts)| facts.suppressed_numbering)
                .map(|(node, facts)| (*node, facts.clone()))
                .collect(),
            bookmark_sources: self.bookmark_sources.clone(),
            resolved_by_bookmark: self.resolved_by_bookmark.clone(),
            main_story_sources: self.main_story_sources.clone(),
            note_labels: self.note_labels.clone(),
            note_body_context: self.note_body_context,
        }
    }

    pub(crate) fn set_note_labels(&mut self, labels: HashMap<NoteRef, String>) {
        self.note_labels = labels;
    }

    pub(crate) fn note_label(&self, note: NoteRef) -> String {
        self.note_labels
            .get(&note)
            .cloned()
            .unwrap_or_else(|| note.id.to_string())
    }

    pub(crate) fn merge_references(&mut self, other: &Self) {
        self.bookmark_sources.extend(other.bookmark_sources.clone());
        self.resolved_by_bookmark
            .extend(other.resolved_by_bookmark.clone());
        self.main_story_sources
            .extend(other.main_story_sources.iter().copied());
    }

    pub(crate) fn bookmark_numbering(
        &self,
    ) -> impl Iterator<Item = (&str, SourceNodeId, &ResolvedNumbering)> {
        self.resolved_by_bookmark
            .iter()
            .map(|(name, (source, numbering))| (name.as_str(), *source, numbering))
    }

    pub(crate) fn take_resolved(&mut self) -> HashMap<SourceNodeId, ResolvedNumbering> {
        std::mem::take(&mut self.resolved_by_source)
    }
}

/// Resolve paragraph properties by walking the style inheritance chain.
pub fn resolve_paragraph_properties(style_id: Option<&str>, styles: &CT_Styles) -> CT_PPr {
    resolve_paragraph_properties_in_table(style_id, styles, None)
}

/// Resolve paragraph properties with a table-style layer between document
/// defaults and the paragraph style.
pub fn resolve_paragraph_properties_in_table(
    style_id: Option<&str>,
    styles: &CT_Styles,
    table_properties: Option<&CT_PPr>,
) -> CT_PPr {
    let mut effective = CT_PPr::default();

    // 1. Start from docDefaults
    if let Some(ref defaults) = styles.doc_defaults
        && let Some(ref ppr) = defaults.ppr
    {
        merge_paragraph_layer(&mut effective, ppr);
    }

    if let Some(properties) = table_properties {
        merge_paragraph_layer(&mut effective, properties);
    }

    // 2. Walk the selected style's basedOn chain
    let selected_style_id = style_id.or_else(|| {
        styles
            .get_default(StyleType::Paragraph)
            .map(|style| style.style_id.as_str())
    });
    if let Some(sid) = selected_style_id {
        let chain = collect_style_chain(sid, styles);
        // Apply from most-base to most-derived
        for style in chain.iter().rev() {
            if let Some(ref ppr) = style.ppr {
                merge_paragraph_layer(&mut effective, ppr);
            }
        }
    }

    effective
}

/// Merge one layer of paragraph properties, with Word's rule for tab stops.
fn merge_paragraph_layer(effective: &mut CT_PPr, layer: &CT_PPr) {
    let inherited_tabs = effective.tabs.take();
    effective.merge_from(layer);
    effective.tabs = inherited_tabs;
    merge_tab_stops(&mut effective.tabs, layer.tabs.as_ref());
}

/// Apply one layer of `w:tabs` to the tab stops inherited so far.
///
/// Word adds a layer's stops to the ones a paragraph inherits from its
/// styles and its list level rather than replacing them. A stop replaces an
/// inherited stop at the same position, and a `clear` stop removes it. Word
/// reads the stops of a single layer in their listed order, and sorts stops
/// merged from several layers.
pub(crate) fn merge_tab_stops(inherited: &mut Option<CT_Tabs>, layer: Option<&CT_Tabs>) {
    let Some(layer) = layer else {
        return;
    };
    let merged = inherited.get_or_insert_with(CT_Tabs::default);
    let sort = !merged.tabs.is_empty();
    // The last stop a layer sets at a position is the one that counts.
    let last = layer
        .tabs
        .iter()
        .enumerate()
        .map(|(index, tab)| (tab.pos.0, index))
        .collect::<HashMap<_, _>>();
    merged
        .tabs
        .retain(|existing| !last.contains_key(&existing.pos.0));
    merged.tabs.extend(
        layer
            .tabs
            .iter()
            .enumerate()
            .filter(|(index, tab)| last[&tab.pos.0] == *index && tab.val != ST_TabJc::Clear)
            .map(|(_, tab)| tab.clone()),
    );
    if sort {
        merged.tabs.sort_by_key(|tab| tab.pos.0);
    }
}

/// Resolve run properties by walking paragraph and character style chains.
///
/// `table_properties` is the run layer a table style resolved for the cell the
/// paragraph sits in, and is `None` everywhere outside a styled table. It
/// merges after the document defaults and before the paragraph style, which is
/// the ordering `resolve_paragraph_properties_in_table` already uses.
pub fn resolve_run_properties(
    para_style_id: Option<&str>,
    run_style_id: Option<&str>,
    styles: &CT_Styles,
    table_properties: Option<&CT_RPr>,
) -> CT_RPr {
    let mut effective = CT_RPr::default();

    // 1. docDefaults run properties
    if let Some(ref defaults) = styles.doc_defaults
        && let Some(ref rpr) = defaults.rpr
    {
        effective.merge_from(rpr);
    }

    if let Some(properties) = table_properties {
        effective.merge_from(properties);
    }

    // 2. paragraph style's rpr (following basedOn chain)
    let para_sid = para_style_id.or_else(|| {
        styles
            .get_default(StyleType::Paragraph)
            .map(|s| s.style_id.as_str())
    });
    if let Some(sid) = para_sid {
        let chain = collect_style_chain(sid, styles);
        for style in chain.iter().rev() {
            if let Some(ref rpr) = style.rpr {
                effective.merge_from(rpr);
            }
        }
    }

    // 3. character style's rpr (following basedOn chain)
    if let Some(sid) = run_style_id.and_then(|style_id| character_style_id(style_id, styles)) {
        let chain = collect_style_chain(sid, styles);
        for style in chain.iter().rev() {
            if let Some(ref rpr) = style.rpr {
                effective.merge_from(rpr);
            }
        }
    }

    effective
}

/// Generated section labels use named Word roles, independently of body defaults.
pub(crate) fn line_number_properties(
    styles: &CT_Styles,
    diagnostics: &mut Vec<oxoffice_layout::Diagnostic>,
) -> CT_RPr {
    let defaults = styles
        .doc_defaults
        .as_ref()
        .and_then(|defaults| defaults.rpr.as_ref());
    let normal = line_number_style_level(styles, "Normal", StyleType::Paragraph, diagnostics);
    let character =
        line_number_style_level(styles, "line number", StyleType::Character, diagnostics);
    let mut effective = defaults.cloned().unwrap_or_default();
    effective.merge_from(&normal);
    effective.merge_from(&character);
    let toggle = |default: Option<bool>, paragraph: Option<bool>, character: Option<bool>| {
        Some(default == Some(true) || (paragraph.unwrap_or(false) ^ character.unwrap_or(false)))
    };
    effective.bold = toggle(
        defaults.and_then(|rpr| rpr.bold),
        normal.bold,
        character.bold,
    );
    effective.italic = toggle(
        defaults.and_then(|rpr| rpr.italic),
        normal.italic,
        character.italic,
    );
    effective
}

fn line_number_style_level(
    styles: &CT_Styles,
    name: &str,
    kind: StyleType,
    diagnostics: &mut Vec<oxoffice_layout::Diagnostic>,
) -> CT_RPr {
    let named = styles
        .styles
        .iter()
        .filter(|style| {
            style
                .name
                .as_deref()
                .is_some_and(|value| value.trim() == name)
        })
        .collect::<Vec<_>>();
    if named.iter().any(|style| style.style_type != kind) {
        diagnostics.push(oxoffice_layout::Diagnostic {
            message: format!("line-number style role {name:?} has a wrong-type declaration"),
        });
    }
    let candidates = named
        .into_iter()
        .filter(|style| style.style_type == kind)
        .collect::<Vec<_>>();
    if candidates.len() > 1 {
        diagnostics.push(oxoffice_layout::Diagnostic {
            message: format!("line-number style role {name:?} is ambiguous"),
        });
        return CT_RPr::default();
    }
    let mut current = candidates.first().copied();
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    while let Some(style) = current {
        if matches!(
            style.style_id.as_str(),
            "DefaultParagraphFont" | "NoList" | "TableNormal"
        ) {
            break;
        }
        if !seen.insert(style.style_id.as_str()) {
            diagnostics.push(oxoffice_layout::Diagnostic {
                message: format!("line-number style role {name:?} has a basedOn cycle"),
            });
            break;
        }
        chain.push(style);
        let Some(parent) = style.based_on.as_deref() else {
            break;
        };
        match styles.get_by_id(parent) {
            Some(parent) if parent.style_type == kind => current = Some(parent),
            _ => {
                diagnostics.push(oxoffice_layout::Diagnostic {
                    message: format!(
                        "line-number style role {name:?} has a missing or wrong-type basedOn owner"
                    ),
                });
                break;
            }
        }
    }
    let mut properties = CT_RPr::default();
    for style in chain.into_iter().rev() {
        if let Some(rpr) = &style.rpr {
            properties.merge_from(rpr);
        }
    }
    properties
}

pub(crate) fn word_kerning_enabled(properties: &CT_RPr) -> bool {
    properties
        .kern
        .is_some_and(|threshold| threshold.0 <= properties.sz.map_or(22, |size| size.0))
}

fn character_style_id<'a>(style_id: &'a str, styles: &'a CT_Styles) -> Option<&'a str> {
    let style = styles.get_by_id(style_id)?;
    match style.style_type {
        StyleType::Character => Some(style.style_id.as_str()),
        StyleType::Paragraph => style.linked_style.as_deref().filter(|linked_id| {
            styles
                .get_by_id(linked_id)
                .is_some_and(|linked| linked.style_type == StyleType::Character)
        }),
        StyleType::Table | StyleType::Numbering => None,
    }
}

/// The paragraph properties a numbering level carries, mainly its indentation.
///
/// In the property chain these sit between the paragraph style and direct
/// formatting, so the indent for a list level applies unless the paragraph
/// sets its own.
pub fn level_paragraph_properties(
    num_id: u32,
    ilvl: u32,
    numbering: &CT_Numbering,
) -> Option<&CT_PPr> {
    let (_, level, _) = resolved_numbering_level(num_id, ilvl, numbering)?;
    level.ppr.as_ref()
}

fn resolved_numbering_level(
    num_id: u32,
    ilvl: u32,
    numbering: &CT_Numbering,
) -> Option<(&CT_Lvl, &CT_Lvl, Option<u32>)> {
    if num_id == 0 {
        return None;
    }
    let instance = numbering.nums.iter().find(|item| item.num_id == num_id)?;
    let definition = numbering
        .abstract_nums
        .iter()
        .find(|item| item.abstract_num_id == instance.abstract_num_id)?;
    let base = definition.levels.iter().find(|level| level.ilvl == ilvl)?;
    let level_override = instance
        .level_overrides
        .iter()
        .find(|value| value.ilvl == ilvl);
    let effective = level_override
        .and_then(|value| value.level.as_ref())
        .unwrap_or(base);
    Some((
        base,
        effective,
        level_override.and_then(|value| value.start_override),
    ))
}

/// Generate the marker text for a numbered/bulleted list item.
pub fn generate_marker(
    num_id: u32,
    ilvl: u32,
    numbering: &CT_Numbering,
    state: &mut NumberingState,
) -> Option<ResolvedNumbering> {
    let (base_lvl, lvl, start_override) = resolved_numbering_level(num_id, ilvl, numbering)?;
    let num_fmt = lvl.num_fmt.clone().unwrap_or(ST_NumberFormat::Decimal);
    let numbering_instance = numbering.nums.iter().find(|item| item.num_id == num_id)?;
    let definition_id = numbering_instance.abstract_num_id;
    let has_start_override = numbering_instance
        .level_overrides
        .iter()
        .any(|level| level.start_override.is_some());
    let start = base_lvl.start.unwrap_or(1);
    let lvl_text = lvl.lvl_text.as_deref().unwrap_or("%1.");
    let legal = lvl.legal == Some(true);

    let count = state.advance_numbering(
        num_id,
        definition_id,
        ilvl,
        start,
        start_override,
        numbering,
    );
    if matches!(num_fmt, ST_NumberFormat::None | ST_NumberFormat::Other(_)) {
        return None;
    }
    let displayed_num_fmt = if legal && num_fmt != ST_NumberFormat::Bullet {
        ST_NumberFormat::Decimal
    } else {
        num_fmt.clone()
    };
    if displayed_num_fmt != ST_NumberFormat::Bullet
        && !number_format_has_visible_renderer(&displayed_num_fmt)
    {
        return None;
    }
    for level in 0..=ilvl {
        if !lvl_text.contains(&format!("%{}", level + 1)) {
            continue;
        }
        let format = if legal {
            ST_NumberFormat::Decimal
        } else {
            resolved_numbering_level(num_id, level, numbering)
                .and_then(|(_, level, _)| level.num_fmt.clone())
                .unwrap_or(ST_NumberFormat::Decimal)
        };
        if !number_format_has_visible_renderer(&format) {
            return None;
        }
    }

    let number_current = match &displayed_num_fmt {
        ST_NumberFormat::Bullet => String::new(),
        ST_NumberFormat::None | ST_NumberFormat::Other(_) => unreachable!("returned above"),
        _ => format_number(count, displayed_num_fmt.clone()),
    };
    let marker_text = match displayed_num_fmt {
        ST_NumberFormat::Bullet => lvl_text.to_string(),
        ST_NumberFormat::None | ST_NumberFormat::Other(_) => unreachable!("returned above"),
        _ => format_lvl_text(lvl_text, num_id, ilvl, count, numbering, state, legal),
    };
    let number_context = (0..=ilvl)
        .map(|level| {
            let value = if level == ilvl {
                count
            } else {
                let configured_start = resolved_numbering_level(num_id, level, numbering)
                    .map(|(base, _, _)| base.start.unwrap_or(1))
                    .unwrap_or(0);
                state
                    .counters
                    .get(&(definition_id, level))
                    .copied()
                    .unwrap_or(configured_start)
            };
            let format = if legal && level <= ilvl {
                ST_NumberFormat::Decimal
            } else {
                resolved_numbering_level(num_id, level, numbering)
                    .and_then(|(_, level, _)| level.num_fmt.clone())
                    .unwrap_or(ST_NumberFormat::Decimal)
            };
            format_number(value, format)
        })
        .collect::<Vec<_>>();
    let number_level = marker_text.trim_end_matches('.').to_owned();
    let number_level_without_text = format_lvl_text_without_literals(lvl_text, &number_context);
    let number_level_has_ancestor =
        (0..ilvl).any(|level| lvl_text.contains(&format!("%{}", level + 1)));
    let number_delimiters = contextual_number_delimiters(num_id, ilvl, numbering);
    let trailing_delimiter = trailing_number_delimiter(lvl_text);
    let embedded_start = (0..=ilvl)
        .find(|level| lvl_text.contains(&format!("%{}", level + 1)))
        .unwrap_or(ilvl) as usize;
    let number_suffixes_without_text = (0..number_context.len())
        .map(|start| {
            if embedded_start == 0 {
                let mut result = number_context[start].clone();
                for level in start + 1..number_context.len() {
                    result.push_str(&number_delimiters[level - 1]);
                    result.push_str(&number_context[level]);
                }
                result.push_str(&trailing_delimiter);
                return result.trim_end_matches('.').to_owned();
            }
            let mut result = String::new();
            if start < embedded_start {
                result.push_str(&number_context[start]);
                for level in start + 1..embedded_start {
                    result.push_str(&number_delimiters[level - 1]);
                    result.push_str(&number_context[level]);
                }
                result.push_str(&number_delimiters[embedded_start - 1]);
            }
            result.push_str(&number_level_without_text);
            result
        })
        .collect::<Vec<_>>();
    let number_full_without_text = number_suffixes_without_text
        .first()
        .cloned()
        .unwrap_or_default();
    let number_full = full_number_keeping_literals(num_id, ilvl, numbering, &number_context);

    let marker_rpr = lvl.rpr.clone().unwrap_or_default();

    Some(ResolvedNumbering {
        number_full,
        marker_text,
        number_current,
        number_level,
        number_level_without_text,
        number_level_has_ancestor,
        number_full_without_text,
        number_context,
        number_suffixes_without_text,
        num_id,
        definition_id,
        has_start_override,
        marker_rpr,
        suffix: lvl.suffix.unwrap_or(ST_LvlSuffix::Tab),
    })
}

fn format_lvl_text_keeping_literals(template: &str, context: &[String]) -> String {
    let mut result = String::new();
    let mut chars = template.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '%'
            && let Some(digit @ '1'..='9') = chars.peek().copied()
        {
            chars.next();
            let level = digit as usize - '1' as usize;
            if let Some(value) = context.get(level) {
                result.push_str(value);
            }
        } else {
            result.push(character);
        }
    }
    result
}

/// Build `\w` by keeping each level's literal text.
///
/// A level whose template already names every ancestor placeholder replaces
/// the accumulated path. Any other level is appended. One trailing full stop
/// is removed from the finished path.
fn full_number_keeping_literals(
    num_id: u32,
    ilvl: u32,
    numbering: &CT_Numbering,
    context: &[String],
) -> String {
    let mut accumulated = String::new();
    for level in 0..=ilvl {
        let Some((_, lvl, _)) = resolved_numbering_level(num_id, level, numbering) else {
            continue;
        };
        let template = lvl.lvl_text.as_deref().unwrap_or("%1.");
        let formatted = format_lvl_text_keeping_literals(template, context);
        let contains_every_ancestor =
            (0..level).all(|ancestor| template.contains(&format!("%{}", ancestor + 1)));
        if contains_every_ancestor {
            accumulated = formatted;
        } else {
            accumulated.push_str(&formatted);
        }
    }
    if accumulated.ends_with('.') {
        accumulated.pop();
    }
    accumulated
}

fn format_lvl_text_without_literals(template: &str, context: &[String]) -> String {
    let mut result = String::new();
    let mut chars = template.chars().peekable();
    while let Some(character) = chars.next() {
        if character == '%'
            && let Some(digit @ '1'..='9') = chars.peek().copied()
        {
            chars.next();
            let level = digit as usize - '1' as usize;
            if let Some(value) = context.get(level) {
                result.push_str(value);
            }
        } else if is_number_delimiter(character) {
            result.push(character);
        }
    }
    result.trim_end_matches('.').to_owned()
}

fn is_number_delimiter(character: char) -> bool {
    !character.is_alphanumeric() && !character.is_whitespace()
}

fn trailing_number_delimiter(template: &str) -> String {
    let bytes = template.as_bytes();
    let last_placeholder_end = (0..bytes.len().saturating_sub(1))
        .filter(|index| bytes[*index] == b'%' && (b'1'..=b'9').contains(&bytes[*index + 1]))
        .map(|index| index + 2)
        .next_back();
    template
        .get(last_placeholder_end.unwrap_or(template.len())..)
        .unwrap_or_default()
        .chars()
        .filter(|character| is_number_delimiter(*character))
        .collect()
}

fn contextual_number_delimiters(num_id: u32, ilvl: u32, numbering: &CT_Numbering) -> Vec<String> {
    (1..=ilvl)
        .map(|right_level| {
            for candidate_level in (right_level..=ilvl).rev() {
                let Some((_, level, _)) =
                    resolved_numbering_level(num_id, candidate_level, numbering)
                else {
                    continue;
                };
                let Some(template) = level.lvl_text.as_deref() else {
                    continue;
                };
                let left = format!("%{right_level}");
                let right = format!("%{}", right_level + 1);
                let Some(left_start) = template.find(&left) else {
                    continue;
                };
                let Some(right_start) = template[left_start + left.len()..]
                    .find(&right)
                    .map(|offset| left_start + left.len() + offset)
                else {
                    continue;
                };
                let delimiter = template[left_start + left.len()..right_start]
                    .chars()
                    .filter(|character| is_number_delimiter(*character))
                    .collect::<String>();
                if !delimiter.is_empty() {
                    return delimiter;
                }
            }

            let left_level = right_level - 1;
            let Some((_, level, _)) = resolved_numbering_level(num_id, left_level, numbering)
            else {
                return String::new();
            };
            let Some(template) = level.lvl_text.as_deref() else {
                return String::new();
            };
            let placeholder = format!("%{}", left_level + 1);
            template
                .rfind(&placeholder)
                .map(|start| &template[start + placeholder.len()..])
                .unwrap_or_default()
                .chars()
                .filter(|character| is_number_delimiter(*character))
                .collect()
        })
        .collect()
}

/// Format level text by substituting %1, %2, etc. with formatted counters.
fn format_lvl_text(
    template: &str,
    num_id: u32,
    current_ilvl: u32,
    current_count: u32,
    numbering: &CT_Numbering,
    state: &NumberingState,
    legal: bool,
) -> String {
    let mut result = template.to_string();
    for lvl_idx in 0..=8u32 {
        let placeholder = format!("%{}", lvl_idx + 1);
        if result.contains(&placeholder) {
            let count = if lvl_idx == current_ilvl {
                current_count
            } else {
                let configured_start = resolved_numbering_level(num_id, lvl_idx, numbering)
                    .map(|(base, _, _)| base.start.unwrap_or(1))
                    .unwrap_or(0);
                let owner = state
                    .definition_by_instance
                    .get(&num_id)
                    .copied()
                    .unwrap_or(num_id);
                state
                    .counters
                    .get(&(owner, lvl_idx))
                    .copied()
                    .unwrap_or(configured_start)
            };
            let fmt = if legal && lvl_idx <= current_ilvl {
                ST_NumberFormat::Decimal
            } else {
                resolved_numbering_level(num_id, lvl_idx, numbering)
                    .and_then(|(_, level, _)| level.num_fmt.clone())
                    .unwrap_or(ST_NumberFormat::Decimal)
            };
            let formatted = format_number(count, fmt);
            result = result.replace(&placeholder, &formatted);
        }
    }
    result
}

/// Format a number according to ST_NumberFormat.
pub(crate) fn format_number(n: u32, fmt: ST_NumberFormat) -> String {
    match fmt {
        ST_NumberFormat::Decimal => n.to_string(),
        ST_NumberFormat::UpperRoman => to_roman(n, true),
        ST_NumberFormat::LowerRoman => to_roman(n, false),
        ST_NumberFormat::UpperLetter => to_letter(n, true),
        ST_NumberFormat::LowerLetter => to_letter(n, false),
        ST_NumberFormat::Bullet | ST_NumberFormat::None | ST_NumberFormat::Other(_) => {
            String::new()
        }
        // Every standard OOXML token is typed and round-trippable.
        // Rendering algorithms beyond the established decimal, letter, and
        // Roman families remain outside this package-level story.
        _ => String::new(),
    }
}

fn number_format_has_visible_renderer(format: &ST_NumberFormat) -> bool {
    matches!(
        format,
        ST_NumberFormat::Decimal
            | ST_NumberFormat::UpperRoman
            | ST_NumberFormat::LowerRoman
            | ST_NumberFormat::UpperLetter
            | ST_NumberFormat::LowerLetter
    )
}

fn to_roman(mut n: u32, upper: bool) -> String {
    let vals = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut result = String::new();
    for &(value, numeral) in &vals {
        while n >= value {
            result.push_str(numeral);
            n -= value;
        }
    }
    if upper { result } else { result.to_lowercase() }
}

fn to_letter(n: u32, upper: bool) -> String {
    if n == 0 {
        return String::new();
    }
    let base = if upper { b'A' } else { b'a' };
    let idx = ((n - 1) % 26) as u8;
    String::from(char::from(base + idx))
}

/// Collect the style chain from the given style up through basedOn ancestors.
fn collect_style_chain<'a>(style_id: &str, styles: &'a CT_Styles) -> Vec<&'a CT_Style> {
    let mut chain = Vec::new();
    let mut current_id = Some(style_id.to_string());
    let mut seen = HashSet::new();

    while let Some(ref sid) = current_id {
        if !seen.insert(sid.clone()) {
            break; // Prevent cycles
        }
        if let Some(style) = styles.get_by_id(sid) {
            chain.push(style);
            current_id = style.based_on.clone();
        } else {
            break;
        }
    }

    chain
}

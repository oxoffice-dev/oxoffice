//! Reading a document's structure: headings and outlines, images, links, word count and accessibility.

use super::*;

impl Document {
    // ---- Document Intelligence API ----

    /// Get all headings in the document as (level, text) pairs.
    ///
    /// Detects heading paragraphs by their style's built-in name (`heading
    /// 1`, `heading 2`...), which Word keeps when it saves in another language
    /// and localises the id, else by an id `Heading1`, `Heading2`..., among the
    /// body paragraphs, including those that body-level content
    /// controls wrap. Table cells are not searched.
    pub fn headings(&self) -> Vec<(u32, String)> {
        let mut result = Vec::new();
        for p in block_paragraphs(&self.document.body.content) {
            if let Some(level) = self.detect_heading_level_for_toc(p) {
                result.push((level, p.text()));
            }
        }
        result
    }

    /// Get a hierarchical outline of the document headings.
    ///
    /// Returns a tree structure where each node contains the heading level,
    /// text, and children (sub-headings).
    pub fn document_outline(&self) -> Vec<OutlineNode> {
        let headings = self.headings();
        build_outline_tree(&headings)
    }

    /// Accepted source-qualified headings, including tables and nested controls.
    /// Legacy title-only heading accessors retain their established scope.
    pub fn numbered_document_outline(&self) -> Result<Vec<NumberedOutlineNode>> {
        let layout = self.layout_deterministic()?;
        if let Some(failure) = layout.navigation_failure() {
            return Err(Error::Other(format!(
                "numbering-aware navigation unavailable: {failure}"
            )));
        }
        let mut roots: Vec<NumberedOutlineNode> = Vec::new();
        let mut stack: Vec<(u32, usize)> = Vec::new();
        for (_, source, level, text, _, numbering, _) in layout.paragraph_navigation() {
            let Some(level) =
                level.filter(|_| source.story == oxoffice_docx_layout::WordStory::Document)
            else {
                continue;
            };
            let number = numbering
                .filter(|number| number.marker_rpr.vanish != Some(true))
                .map(|number| number.marker_text.clone())
                .filter(|v| !v.is_empty());
            let display_text = match number.as_deref() {
                Some(number) if !text.is_empty() => format!("{number} {text}"),
                Some(number) => number.to_owned(),
                None => text.to_owned(),
            };
            let node = NumberedOutlineNode {
                level,
                text: text.to_owned(),
                number,
                display_text,
                source: source.clone(),
                children: Vec::new(),
            };
            while stack.last().is_some_and(|(parent, _)| *parent >= level) {
                stack.pop();
            }
            let mut children = &mut roots;
            for &(_, index) in &stack {
                children = &mut children[index].children;
            }
            let index = children.len();
            children.push(node);
            stack.push((level, index));
        }
        Ok(roots)
    }

    /// Get information about all images in the document.
    ///
    /// Returns metadata for each inline and anchored image found in body
    /// paragraphs, table cells and content controls, in document order.
    pub fn images(&self) -> Vec<ImageInfo> {
        let mut result = Vec::new();
        visit_all_drawings(&self.document.body.content, &mut |drawing| {
            if let Some(inline) = &drawing.inline {
                result.push(ImageInfo {
                    embed_id: inline.embed_id.clone(),
                    name: inline.name.clone(),
                    description: inline.description.clone(),
                    width_emu: inline.extent_cx.0,
                    height_emu: inline.extent_cy.0,
                    is_anchor: false,
                });
            }
            if let Some(anchor) = &drawing.anchor {
                result.push(ImageInfo {
                    embed_id: anchor.embed_id.clone(),
                    name: anchor.name.clone(),
                    description: anchor.description.clone(),
                    width_emu: anchor.extent_cx.0,
                    height_emu: anchor.extent_cy.0,
                    is_anchor: true,
                });
            }
        });
        result
    }

    /// Get information about all hyperlinks in the document.
    ///
    /// Reads the body paragraphs, including those that body-level content
    /// controls wrap. Table cells are not searched.
    /// Resolves hyperlink relationship IDs to their target URLs where possible.
    pub fn links(&self) -> Vec<LinkInfo> {
        use oxoffice_opc::relationship::rel_types;

        // Build a map of hyperlink rel_id -> target URL
        let mut url_map = std::collections::HashMap::new();
        if let Some(rels) = self.package.get_part_rels(&self.doc_part_name) {
            for rel in &rels.items {
                if rel.rel_type == rel_types::HYPERLINK
                    && rel.target_mode.as_ref().is_some_and(|m| m == "External")
                {
                    url_map.insert(rel.id.clone(), rel.target.clone());
                }
            }
        }

        let mut result = Vec::new();
        for p in block_paragraphs(&self.document.body.content) {
            for hl in &p.hyperlinks {
                // `HyperlinkSpan`'s bounds are public and can be set by
                // hand, so clamp rather than slice-panic on a bad range.
                let start = hl.run_start.min(p.runs.len());
                let end = hl.run_end.clamp(start, p.runs.len());
                let text: String = p.runs[start..end].iter().map(|r| r.text()).collect();

                let url = hl.rel_id.as_ref().and_then(|id| url_map.get(id)).cloned();

                result.push(LinkInfo {
                    text,
                    url,
                    anchor: hl.anchor.clone(),
                    rel_id: hl.rel_id.clone(),
                });
            }
            for field in p.complex_field_hyperlinks() {
                let start = field.run_start.min(p.runs.len());
                let end = field.run_end.clamp(start, p.runs.len());
                let text: String = p.runs[start..end].iter().map(|run| run.text()).collect();
                result.push(LinkInfo {
                    text,
                    url: Some(field.target),
                    anchor: None,
                    rel_id: None,
                });
            }
        }
        result
    }

    /// Count the number of words in the document.
    ///
    /// Counts whitespace-separated tokens across all paragraphs (including
    /// paragraphs inside table cells and content controls), leaving out the
    /// table rows that accepting every tracked change removes, as
    /// [`Self::text`] does.
    pub fn word_count(&self) -> usize {
        let mut count = 0;
        visit_body_paragraphs(&self.document.body.content, true, &mut |paragraph| {
            count += paragraph.text().split_whitespace().count();
        });
        count
    }

    /// Audit the document for accessibility issues.
    ///
    /// Checks for common problems: missing image alt text, heading level gaps,
    /// empty paragraphs, missing document metadata.
    pub fn audit_accessibility(&self) -> Vec<AccessibilityIssue> {
        let mut issues = Vec::new();

        // Check: missing document title
        if self.title().is_none() {
            issues.push(AccessibilityIssue {
                severity: IssueSeverity::Warning,
                message: "Document has no title".to_string(),
            });
        }

        // Check: missing document language (author as a proxy for basic metadata)
        if self.author().is_none() {
            issues.push(AccessibilityIssue {
                severity: IssueSeverity::Info,
                message: "Document has no author".to_string(),
            });
        }

        // Check: images without alt text
        let images = self.images();
        for img in &images {
            let has_alt = img
                .description
                .as_ref()
                .is_some_and(|d| !d.is_empty() && d != "Background");
            if !has_alt {
                let name = img
                    .name
                    .as_deref()
                    .or(Some(&img.embed_id))
                    .unwrap_or("unknown");
                issues.push(AccessibilityIssue {
                    severity: IssueSeverity::Error,
                    message: format!("Image \"{name}\" has no alt text"),
                });
            }
        }

        // Check: heading level gaps
        let headings = self.headings();
        let mut prev_level: Option<u32> = None;
        for (level, text) in &headings {
            if let Some(prev) = prev_level
                && *level > prev + 1
            {
                issues.push(AccessibilityIssue {
                    severity: IssueSeverity::Warning,
                    message: format!(
                        "Heading level gap: h{prev} -> h{level} (\"{}\")",
                        truncate_str(text, 40)
                    ),
                });
            }
            prev_level = Some(*level);
        }

        // Check direct body streaks only. An uncertain source omits this whole check.
        let visibility = (|| {
            let mut candidate = self.clone_for_staging();
            candidate.flush_document_to_package()?;
            let source = candidate
                .package
                .get_part(&candidate.doc_part_name)
                .ok_or(Error::NoDocumentPart)?;
            paragraph_visibility_from_main_source(
                &candidate,
                source,
                ParagraphVisibilityScope::DirectBody,
            )
        })();
        match visibility {
            Ok(visibility) => {
                let mut flags = visibility.into_iter();
                let mut consecutive_empty = 0u32;
                for content in &self.document.body.content {
                    if let BodyContent::Paragraph(_) = content {
                        if flags.next() == Some(false) {
                            consecutive_empty += 1;
                            if consecutive_empty >= 3 {
                                issues.push(AccessibilityIssue { severity: IssueSeverity::Info,
                                    message: format!("{consecutive_empty} consecutive empty paragraphs (consider using spacing instead)") });
                            }
                        } else {
                            consecutive_empty = 0;
                        }
                    } else {
                        consecutive_empty = 0;
                    }
                }
            }
            Err(error) => issues.push(AccessibilityIssue {
                severity: IssueSeverity::Warning,
                message: format!("Paragraph visibility could not be determined: {error}"),
            }),
        }

        issues
    }
}

/// A node in the document outline tree.
#[derive(Debug, Clone, PartialEq)]
pub struct OutlineNode {
    /// The heading level (1-9).
    pub level: u32,
    /// The heading text.
    pub text: String,
    /// Child headings (sub-headings).
    pub children: Vec<OutlineNode>,
}

/// Information about an image in the document.
#[derive(Debug, Clone, PartialEq)]
pub struct ImageInfo {
    /// The relationship ID for the embedded image.
    pub embed_id: String,
    /// Optional name attribute.
    pub name: Option<String>,
    /// Optional description (alt text).
    pub description: Option<String>,
    /// Width in EMUs (English Metric Units, 914400 EMU = 1 inch).
    pub width_emu: i64,
    /// Height in EMUs.
    pub height_emu: i64,
    /// Whether this is an anchored (floating) image vs inline.
    pub is_anchor: bool,
}

/// Information about a hyperlink in the document.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkInfo {
    /// The display text of the hyperlink.
    pub text: String,
    /// The resolved target URL (if external).
    pub url: Option<String>,
    /// Internal document anchor (if any).
    pub anchor: Option<String>,
    /// The relationship ID.
    pub rel_id: Option<String>,
}

/// Severity level for accessibility issues.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IssueSeverity {
    /// Informational suggestion.
    Info,
    /// Potential problem.
    Warning,
    /// Definite accessibility barrier.
    Error,
}

/// An accessibility issue found during audit.
#[derive(Debug, Clone, PartialEq)]
pub struct AccessibilityIssue {
    /// How severe the issue is.
    pub severity: IssueSeverity,
    /// Human-readable description of the issue.
    pub message: String,
}

/// One accepted heading with its resolved visible marker and physical source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NumberedOutlineNode {
    pub level: u32,
    pub text: String,
    pub number: Option<String>,
    pub display_text: String,
    pub source: oxoffice_docx_layout::WordSourcePath,
    pub children: Vec<NumberedOutlineNode>,
}

/// Build a hierarchical outline tree from a flat list of (level, text) headings.
fn build_outline_tree(headings: &[(u32, String)]) -> Vec<OutlineNode> {
    let mut root: Vec<OutlineNode> = Vec::new();
    let mut stack: Vec<(u32, usize)> = Vec::new(); // (level, index in parent's children)

    for (level, text) in headings {
        let node = OutlineNode {
            level: *level,
            text: text.clone(),
            children: Vec::new(),
        };

        // Pop stack until we find a parent with a lower level
        while let Some(&(stack_level, _)) = stack.last() {
            if stack_level >= *level {
                stack.pop();
            } else {
                break;
            }
        }

        if stack.is_empty() {
            root.push(node);
            let idx = root.len() - 1;
            stack.push((*level, idx));
        } else {
            // Navigate to the correct parent in the tree
            let target = get_outline_parent_mut(&mut root, &stack);
            target.children.push(node);
            let idx = target.children.len() - 1;
            stack.push((*level, idx));
        }
    }

    root
}

/// Navigate to the parent node indicated by the stack.
fn get_outline_parent_mut<'a>(
    root: &'a mut [OutlineNode],
    stack: &[(u32, usize)],
) -> &'a mut OutlineNode {
    let mut current = &mut root[stack[0].1];
    for &(_, idx) in &stack[1..] {
        current = &mut current.children[idx];
    }
    current
}

/// Truncate a string to at most `max_len` characters, appending "..." if it
/// was cut short.
///
/// Both the comparison and the cut are in characters; mixing byte length with
/// character counts would truncate non-ASCII text earlier than asked.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max_len.saturating_sub(3)).collect();
    format!("{truncated}...")
}

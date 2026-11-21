//! Ordered replacements that refuse to leave their text behind.

use super::{Document, ReplacementCountMismatch, StoryKind, StoryParagraph};
use std::collections::HashMap;

use crate::error::Result;

/// How [`Document::try_replace_all_checked`] treats a text that is still in
/// the document after its replacement.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReplaceOptions {
    /// Publish even when a pair's text is left out of its replacement's
    /// reach. Off by default: such a batch is refused and the document stays
    /// as it was.
    pub allow_unreached: bool,
}

/// Why [`Document::try_replace_all_checked`] left the document unchanged.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReplacementRefusal {
    /// A pair found another count than the one it expects.
    #[error(transparent)]
    Count(#[from] ReplacementCountMismatch),
    /// A pair's text is still in the document after its replacement.
    #[error(transparent)]
    Unreached(#[from] UnreachedReplacement),
    /// A story does not read, so what a pair leaves behind cannot be checked.
    #[error(transparent)]
    Unchecked(#[from] UncheckedReplacement),
}

/// The pair of a [`Document::try_replace_all_checked`] batch whose reach
/// cannot be checked, because a story of the document does not read, such as
/// a header whose relationship targets a missing part.
/// [`ReplaceOptions::allow_unreached`] replaces without the check.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "pair {index}: what the replacement of \"{placeholder}\" leaves behind cannot be checked, as a story does not read: {reason}"
)]
pub struct UncheckedReplacement {
    /// Zero-based position of the pair in the batch.
    pub index: usize,
    /// The literal text the pair searched for.
    pub placeholder: String,
    /// Why the stories do not read.
    pub reason: String,
}

/// The pair of a [`Document::try_replace_all_checked`] batch whose text is
/// still in the document after its replacement, where the replacement could
/// not reach it: a match across the edge of a content control, a tracked
/// change or a field, or text in a story replacement does not search, such as
/// a text box inside a note.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}", unreached_message(self))]
pub struct UnreachedReplacement {
    /// Zero-based position of the pair in the batch.
    pub index: usize,
    /// The literal text the pair searched for.
    pub placeholder: String,
    /// How many occurrences the replacement left.
    pub left: usize,
    /// The paragraphs that hold the occurrences left, as
    /// [`Document::story_paragraphs`] reads them after the pair.
    pub paragraphs: Vec<StoryParagraph>,
}

/// Name the pair, its count left and the first paragraphs that hold it.
fn unreached_message(unreached: &UnreachedReplacement) -> String {
    let mut message = format!(
        "pair {}: {} occurrence(s) of \"{}\" left after its replacement, out of its reach",
        unreached.index, unreached.left, unreached.placeholder
    );
    for (shown, paragraph) in unreached.paragraphs.iter().enumerate() {
        if shown == 5 {
            message.push_str(&format!(
                "; and {} more paragraph(s)",
                unreached.paragraphs.len() - shown
            ));
            break;
        }
        let text = paragraph.text.chars().take(60).collect::<String>();
        let ellipsis = if text.len() < paragraph.text.len() {
            "..."
        } else {
            ""
        };
        message.push_str(if shown == 0 { ": " } else { "; " });
        message.push_str(paragraph.part_name());
        if let Some(style) = &paragraph.style {
            message.push_str(&format!(" [{style}]"));
        }
        message.push_str(&format!(": \"{text}{ellipsis}\""));
    }
    message
}

impl Document {
    /// [`Document::try_replace_all_expected`], refusing as well a pair whose
    /// text is still in the document after its replacement.
    ///
    /// Each paragraph of [`Document::story_paragraphs`] that replacement
    /// searches (every story but the comments) and that held a pair's text
    /// must read, after the pair, as its text with every occurrence replaced
    /// (left to right, without overlaps). An occurrence it still holds beyond
    /// that is one the replacement could not reach, such as a match across
    /// the edge of a content control, a tracked change or a field, or text in
    /// a story it does not search. A replacement that forms its text again
    /// with its neighbours, or a value equal to its placeholder, is not
    /// refused, and a match no reader sees, such as a nested field's result
    /// inside a field code, cannot hide one that is left. A refused batch
    /// leaves the document unchanged and names the pair, how many
    /// occurrences it left, and the paragraphs that hold them.
    /// `options.allow_unreached` skips the check and publishes. A document
    /// whose stories the check cannot read is refused with
    /// [`ReplacementRefusal::Unchecked`]. Pairs run in order, so a text that a
    /// later pair replaces is checked after the pair that searches for it,
    /// not after the batch.
    pub fn try_replace_all_checked(
        &mut self,
        pairs: &[(&str, &str, Option<usize>)],
        options: ReplaceOptions,
    ) -> Result<std::result::Result<Vec<usize>, ReplacementRefusal>> {
        if pairs.is_empty() {
            return Ok(Ok(Vec::new()));
        }
        let mut candidate = self.clone_for_staging();
        let counts = match candidate.replace_checked_pairs(pairs, options)? {
            Ok(counts) => counts,
            Err(refusal) => return Ok(Err(refusal)),
        };
        self.commit_staged_mutation(candidate);
        Ok(Ok(counts))
    }

    /// Consume an already-private candidate for
    /// [`Document::try_replace_all_checked`], publishing only positive counts.
    #[doc(hidden)]
    #[allow(clippy::type_complexity)]
    pub fn try_replace_all_checked_owned(
        mut self,
        pairs: &[(&str, &str, Option<usize>)],
        options: ReplaceOptions,
    ) -> Result<std::result::Result<Option<(Self, Vec<usize>)>, ReplacementRefusal>> {
        if pairs.is_empty() {
            return Ok(Ok(None));
        }
        let mut origin = self.take_staged_commit_origin();
        let counts = match self.replace_checked_pairs(pairs, options)? {
            Ok(counts) => counts,
            Err(refusal) => return Ok(Err(refusal)),
        };
        if counts.iter().all(|count| *count == 0) {
            return Ok(Ok(None));
        }
        let candidate = Self::finish_staged_candidate(
            &origin.package,
            origin.signature_invalidated,
            &mut origin.normal_engine,
            &mut origin.bundled_engine,
            self,
        );
        Ok(Ok(Some((candidate, counts))))
    }

    fn replace_checked_pairs(
        &mut self,
        pairs: &[(&str, &str, Option<usize>)],
        options: ReplaceOptions,
    ) -> Result<std::result::Result<Vec<usize>, ReplacementRefusal>> {
        let mut counts = Vec::with_capacity(pairs.len());
        for (index, &(placeholder, replacement, expected)) in pairs.iter().enumerate() {
            let check = !options.allow_unreached && !placeholder.is_empty();
            let unchecked = |reason| UncheckedReplacement {
                index,
                placeholder: placeholder.to_owned(),
                reason,
            };
            let before = if check {
                match self.searched_paragraphs_holding(placeholder) {
                    Ok(paragraphs) => Some(paragraphs),
                    Err(reason) => return Ok(Err(unchecked(reason).into())),
                }
            } else {
                None
            };
            let found = self.replace_batch(&[(placeholder, replacement)])?;
            if let Some(expected) = expected
                && expected != found
            {
                return Ok(Err(ReplacementCountMismatch {
                    index,
                    placeholder: placeholder.to_owned(),
                    expected,
                    found,
                }
                .into()));
            }
            counts.push(found);
            let Some(before) = before else {
                continue;
            };
            let after = match self.searched_paragraphs_holding(placeholder) {
                Ok(paragraphs) => paragraphs,
                Err(reason) => return Ok(Err(unchecked(reason).into())),
            };
            // Each paragraph that held the text must read as its text with
            // every occurrence replaced. Comparing paragraph by paragraph,
            // rather than counting over the document, keeps a match no reader
            // sees (a nested field's result inside a field code) from
            // offsetting one left behind, lets a replacement form its text
            // again with its neighbours ("  " to " " on three spaces), and
            // lets a value equal its placeholder.
            let after_by_place = after
                .iter()
                .map(|paragraph| (place(paragraph), paragraph))
                .collect::<HashMap<_, _>>();
            let mut left = 0;
            let mut paragraphs = Vec::new();
            for held in &before {
                let Some(now) = after_by_place.get(&place(held)) else {
                    continue;
                };
                let unreached = left_behind(&held.text, &now.text, placeholder, replacement);
                if unreached > 0 {
                    left += unreached;
                    paragraphs.push((*now).clone());
                }
            }
            if left > 0 {
                return Ok(Err(UnreachedReplacement {
                    index,
                    placeholder: placeholder.to_owned(),
                    left,
                    paragraphs,
                }
                .into()));
            }
        }
        Ok(Ok(counts))
    }

    /// The story paragraphs that replacement searches, every story but the
    /// comments, that hold `text`, or why the stories do not read.
    fn searched_paragraphs_holding(
        &self,
        text: &str,
    ) -> std::result::Result<Vec<StoryParagraph>, String> {
        Ok(self
            .story_paragraphs()
            .map_err(|error| error.to_string())?
            .into_iter()
            .filter(|paragraph| {
                paragraph.kind != StoryKind::Comment && paragraph.text.contains(text)
            })
            .collect())
    }
}

/// How many occurrences of `old` in `held`, a paragraph's text before the
/// pair, the pair left in `now`, its text after.
///
/// `now` should read as `held` with each occurrence (left to right, without
/// overlaps) replaced by `new` or left as it was: the fewest left over every
/// such reading counts, so an ambiguous one, as with a value equal to its
/// placeholder, gets the benefit of the doubt. Replacement reads tabs and
/// breaks as the story view does (#88), so a text no reading explains comes
/// from a change the check does not model: it counts what that text holds
/// beyond a full replacement.
fn left_behind(held: &str, now: &str, old: &str, new: &str) -> usize {
    if now == held.replace(old, new) {
        return 0;
    }
    match fewest_kept(held, now, old, new) {
        Reading::Kept(left) => left,
        Reading::TooMany => by_length(held, now, old, new),
        Reading::Unexplained => beyond_full_replacement(held, now, old, new),
    }
}

/// What [`fewest_kept`] makes of a paragraph.
enum Reading {
    /// The fewest occurrences kept over the readings that explain it.
    Kept(usize),
    /// No reading explains it.
    Unexplained,
    /// Past [`READINGS`] places at once, which only a value that is a prefix
    /// of its placeholder (or the reverse) over many adjacent occurrences
    /// reaches.
    TooMany,
}

/// The occurrences kept, told by how far the text moved: each replacement
/// moves it by the same number of bytes. Exact when the value and the
/// placeholder differ in length, the only case where readings branch.
fn by_length(held: &str, now: &str, old: &str, new: &str) -> usize {
    let total = held.matches(old).count();
    let step = new.len() as i64 - old.len() as i64;
    let moved = now.len() as i64 - held.len() as i64;
    match usize::try_from(if step == 0 { -1 } else { moved / step }) {
        Ok(made) if moved % step == 0 && made <= total => total - made,
        _ => beyond_full_replacement(held, now, old, new),
    }
}

/// What `now` holds of `old` beyond what a full replacement leaves.
fn beyond_full_replacement(held: &str, now: &str, old: &str, new: &str) -> usize {
    now.matches(old)
        .count()
        .saturating_sub(held.replace(old, new).matches(old).count())
}

/// The readings of `now` as `held` with each occurrence of `old` replaced by
/// `new` or kept.
fn fewest_kept(held: &str, now: &str, old: &str, new: &str) -> Reading {
    let mut reached = HashMap::from([(0usize, 0usize)]);
    let mut from = 0;
    for (at, _) in held.match_indices(old) {
        if reached.len() > READINGS {
            return Reading::TooMany;
        }
        // The fewest occurrences kept for each place in `now` reached after
        // the text up to this occurrence.
        let segment = &held[from..at];
        let mut next = HashMap::new();
        for (&position, &kept) in &reached {
            let Some(after) = now
                .get(position..)
                .filter(|rest| rest.starts_with(segment))
                .map(|_| position + segment.len())
            else {
                continue;
            };
            for (written, more) in [(new, 0), (old, 1)] {
                if now[after..].starts_with(written) {
                    let entry = next.entry(after + written.len()).or_insert(usize::MAX);
                    *entry = (*entry).min(kept + more);
                }
            }
        }
        reached = next;
        from = at + old.len();
    }
    let tail = &held[from..];
    reached
        .into_iter()
        .filter(|(position, _)| now.get(*position..) == Some(tail))
        .map(|(_, kept)| kept)
        .min()
        .map_or(Reading::Unexplained, Reading::Kept)
}

/// The places [`fewest_kept`] follows at once before it counts by length.
const READINGS: usize = 256;

/// Where a paragraph is, without the fingerprint of its story, which changes
/// with the bytes of the story when a pair edits it.
fn place(paragraph: &StoryParagraph) -> (StoryKind, &str, usize, &[usize]) {
    let story = paragraph.location.story();
    (
        story.kind(),
        story.part_name(),
        story.owner_index(),
        paragraph.location.index_path(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn past_the_reading_cap_the_readings_stop() {
        // Each occurrence may be kept or deleted, and the readings of the
        // adjacent occurrences multiply until the cap stops them.
        let held = "ACME".repeat(300) + "x";
        let now = "ACME".repeat(300) + "y";
        assert!(matches!(
            fewest_kept(&held, &now, "ACME", ""),
            Reading::TooMany
        ));
        assert!(matches!(
            fewest_kept(&"ACME".repeat(3), "ACME", "ACME", ""),
            Reading::Kept(1)
        ));
    }
}

//! The property setters of a content control of the main part, run in place
//! on the direct body block that holds the control (#477, S4).
//!
//! See `specs/2026-11-24-staged-edits-in-place.md`. The staged route of each
//! setter (`set_content_control_tag` and its siblings) serializes every
//! story, rewrites the part and reopens the package; it stays for the other
//! stories and the cases below, and is the debug oracle of this route.

use super::*;

/// One edit of a content control's properties or type, as the public setters
/// make it.
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContentControlEdit {
    Tag(Option<String>),
    Alias(Option<String>),
    Lock(Option<ContentControlLock>),
    Appearance(Option<ContentControlAppearance>),
    Placeholder(Option<bool>),
    Type(ContentControlTypePayload),
    Binding {
        store_item_id: String,
        xpath: String,
        prefix_mappings: Option<String>,
    },
}

/// Where a content control of the main part sits, counted inside the direct
/// body block that holds it: the block's `slot`, the index among the story
/// owners of the location's kind inside the block (0 for the body), and the
/// index of the control among that owner's items inside the block.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ControlBlock {
    pub slot: usize,
    pub owner_index: usize,
    pub item_index: usize,
}

/// What the in-place route writes once the control is found.
enum BlockEdit {
    Property(PropertyEdit),
    Type(ContentControlTypePayload),
}

impl Document {
    /// Make `edit` on the control at `location` by the staged route of its
    /// public setter.
    #[doc(hidden)]
    pub fn edit_content_control(
        &mut self,
        location: &ContentLocation,
        edit: ContentControlEdit,
    ) -> Result<()> {
        match edit {
            ContentControlEdit::Tag(tag) => self.set_content_control_tag(location, tag.as_deref()),
            ContentControlEdit::Alias(alias) => {
                self.set_content_control_alias(location, alias.as_deref())
            }
            ContentControlEdit::Lock(lock) => self.set_content_control_lock(location, lock),
            ContentControlEdit::Appearance(appearance) => {
                self.set_content_control_appearance(location, appearance)
            }
            ContentControlEdit::Placeholder(placeholder) => {
                self.set_content_control_placeholder(location, placeholder)
            }
            ContentControlEdit::Type(payload) => self.set_content_control_type(location, payload),
            ContentControlEdit::Binding {
                store_item_id,
                xpath,
                prefix_mappings,
            } => self.set_content_control_binding(
                location,
                &store_item_id,
                &xpath,
                prefix_mappings.as_deref(),
            ),
        }
    }

    /// Make `edit` on the control at `location`, which `block` places in the
    /// main body, on that block alone (`EditUnit::BodyBlocks`), and answer
    /// whether the body changed.
    ///
    /// `Ok(None)` changes nothing and leaves the edit to
    /// [`Self::edit_content_control`]: a location outside the main part's
    /// body, table cells and text boxes, a `block` that does not place a
    /// control there, a picture type (its relationship is checked against
    /// the part), a repeating section that wraps its content in a new item
    /// (which takes a fresh id from every story), and a block that does not
    /// read back as it writes. A refusal leaves the document as it was, with
    /// the staged route's error. The location's story fingerprint is not
    /// read: `block` names the control.
    #[doc(hidden)]
    pub fn edit_content_control_in_block(
        &mut self,
        location: &ContentLocation,
        block: ControlBlock,
        edit: &ContentControlEdit,
    ) -> Result<Option<bool>> {
        #[cfg(debug_assertions)]
        let before = self.clone_for_staging();
        let result = self.edit_content_control_in_block_unchecked(location, block, edit);
        #[cfg(debug_assertions)]
        {
            let checked = before.clone_for_staging();
            let staged_location = checked.fingerprinted(location);
            match &result {
                Ok(None) => assert_eq!(
                    self.clone_for_staging().to_bytes().ok(),
                    before.clone_for_staging().to_bytes().ok(),
                    "a content control edit left to the staged route changed the document"
                ),
                _ => self.assert_in_place_matches_staged(
                    before,
                    &result
                        .as_ref()
                        .map(|_| ())
                        .map_err(|error| Error::Other(error.to_string())),
                    |staged| staged.edit_content_control(&staged_location, edit.clone()),
                ),
            }
        }
        result
    }

    fn edit_content_control_in_block_unchecked(
        &mut self,
        location: &ContentLocation,
        block: ControlBlock,
        edit: &ContentControlEdit,
    ) -> Result<Option<bool>> {
        // The setter's own checks first, in its order: they do not read the
        // control.
        let edit = match edit {
            ContentControlEdit::Tag(tag) => {
                reject_control_text("content-control tag", tag.as_deref())?;
                BlockEdit::Property(PropertyEdit::Tag(tag.clone()))
            }
            ContentControlEdit::Alias(alias) => {
                reject_control_text("content-control alias", alias.as_deref())?;
                BlockEdit::Property(PropertyEdit::Alias(alias.clone()))
            }
            ContentControlEdit::Lock(lock) => BlockEdit::Property(PropertyEdit::Lock(*lock)),
            ContentControlEdit::Appearance(appearance) => {
                BlockEdit::Property(PropertyEdit::Appearance(*appearance))
            }
            ContentControlEdit::Placeholder(placeholder) => {
                BlockEdit::Property(PropertyEdit::Placeholder(*placeholder))
            }
            ContentControlEdit::Type(payload) => {
                validate_type_payload(payload)?;
                if matches!(payload, ContentControlTypePayload::Picture { .. }) {
                    return Ok(None);
                }
                BlockEdit::Type(payload.clone())
            }
            ContentControlEdit::Binding {
                store_item_id,
                xpath,
                prefix_mappings,
            } => {
                let mappings_text = prefix_mappings.as_deref().filter(|value| !value.is_empty());
                let mappings = parse_prefix_mappings(mappings_text.unwrap_or(""))?;
                let steps = parse_xpath(xpath, &mappings)?;
                let part_name =
                    resolve_custom_xml_part(&self.package, &self.doc_part_name, store_item_id)?;
                let item = self.package.get_part(&part_name).ok_or_else(|| {
                    Error::Other(format!("custom XML part {part_name} is missing"))
                })?;
                select_xml_element(item, &steps)?;
                BlockEdit::Property(PropertyEdit::DataBinding {
                    prefix_mappings: mappings_text.map(str::to_owned),
                    xpath: xpath.clone(),
                    store_item_id: store_item_id.clone(),
                })
            }
        };
        let story = location.story();
        let placed = match story.kind() {
            StoryKind::Body => story.owner_index() == 0 && block.owner_index == 0,
            StoryKind::TableCell | StoryKind::TextBox => true,
            _ => false,
        };
        if !placed
            || Self::location_is_end(location)
            || location.item_kind() != StoryItemKind::ContentControl
            || location.index_path().len() != 1
            || story.part_name() != self.doc_part_name
        {
            return Ok(None);
        }
        let Some(source) = self.body_block_item(
            block.slot,
            (story.kind(), block.owner_index),
            block.item_index,
            StoryItemKind::ContentControl,
        )?
        else {
            return Ok(None);
        };
        #[cfg(debug_assertions)]
        self.assert_block_names_the_control(location, &source);
        let xml = &source.xml;
        let range = &source.item;
        let updated = match edit {
            BlockEdit::Property(edit) => edit_sdt_property(xml, range, edit)?,
            BlockEdit::Type(payload) => {
                validate_repeating_type_change(xml, range, &payload)?;
                if matches!(payload, ContentControlTypePayload::RepeatingSection) {
                    match repeating_content(xml, range)? {
                        RepeatingContent::Empty => {
                            return Err(Error::Other("repeating section has no item".to_owned()));
                        }
                        RepeatingContent::NotBlock => {
                            return Err(Error::Other(
                                "repeating section content is not a block sequence".to_owned(),
                            ));
                        }
                        RepeatingContent::AlreadyItem => {}
                        RepeatingContent::Wrap => return Ok(None),
                    }
                }
                let markup = type_markup(&payload)?;
                replace_control_type_markup(xml, range, markup.as_bytes())?
            }
        };
        if updated.as_slice() == &xml[range.clone()] {
            return Ok(Some(false));
        }
        // `body_block_item` answered that the body saves after a typed edit.
        // A property edit that keeps the namespace declarations the control
        // had, adding at most the standard w14 or w15 binding on the element
        // it writes (`w15:appearance`), shadows no prefix and makes no owner
        // the save cannot replay, so the answer holds after the swap, which
        // would otherwise ask again over the whole main part at the next
        // edit (debug builds check a kept answer against that).
        let same_declarations = declares_only_word_extensions(&xml[range.clone()], &updated);
        let mut edited = xml.clone();
        edited.splice(range.clone(), updated);
        let Some(parsed) =
            Self::reparse_body_block(&source.head, source.tail_bytes(), source.block_of(&edited))
        else {
            return Ok(None);
        };
        let changed = self.replace_body_block_in_place(block.slot, parsed)?;
        if changed && same_declarations {
            self.keep_body_savable_after_swap();
        }
        Ok(Some(changed))
    }

    /// `location` with the fingerprint of the story it names now, which the
    /// staged route checks.
    #[cfg(debug_assertions)]
    fn fingerprinted(&self, location: &ContentLocation) -> ContentLocation {
        let story = location.story();
        let found = self.stories().ok().and_then(|stories| {
            stories.into_iter().find(|candidate| {
                candidate.kind() == story.kind()
                    && candidate.part_name() == story.part_name()
                    && candidate.owner_index() == story.owner_index()
            })
        });
        match found {
            Some(story) => {
                ContentLocation::new(story, location.item_kind(), location.index_path().to_vec())
            }
            None => location.clone(),
        }
    }

    /// Debug proof that the block route found the control the staged route
    /// finds: the same bytes.
    #[cfg(debug_assertions)]
    fn assert_block_names_the_control(
        &self,
        location: &ContentLocation,
        source: &crate::document::BodyBlockItem,
    ) {
        let location = self.fingerprinted(location);
        let (_, xml, range, _) =
            control_item(self, &location).expect("the staged route finds the control");
        assert_eq!(
            String::from_utf8_lossy(&xml[range]),
            String::from_utf8_lossy(&source.xml[source.item.clone()]),
            "the block route names another control than the staged route"
        );
    }
}

/// Whether `updated` declares the namespaces `original` declares, in the
/// same order, plus at most the standard `w14` and `w15` bindings.
fn declares_only_word_extensions(original: &[u8], updated: &[u8]) -> bool {
    let standard = [
        format!("xmlns:w14=\"{W14_NS}\""),
        format!("xmlns:w15=\"{W15_NS}\""),
    ];
    let mut kept = namespace_declarations(original).into_iter().peekable();
    for declaration in namespace_declarations(updated) {
        if kept.peek() == Some(&declaration) {
            kept.next();
        } else if !standard
            .iter()
            .any(|binding| binding.as_bytes() == declaration)
        {
            return false;
        }
    }
    kept.next().is_none()
}

/// The namespace declarations of `xml` (`xmlns` and `xmlns:p` attributes
/// with their values), in order.
fn namespace_declarations(xml: &[u8]) -> Vec<&[u8]> {
    let mut declarations = Vec::new();
    let mut at = 0;
    while let Some(found) = xml[at..].windows(5).position(|window| window == b"xmlns") {
        let start = at + found;
        let Some(quote) = xml[start..]
            .iter()
            .position(|byte| matches!(byte, b'"' | b'\''))
        else {
            break;
        };
        let quote_at = start + quote;
        let Some(close) = xml[quote_at + 1..]
            .iter()
            .position(|byte| *byte == xml[quote_at])
        else {
            break;
        };
        let end = quote_at + 1 + close + 1;
        declarations.push(&xml[start..end]);
        at = end;
    }
    declarations
}

#[cfg(test)]
mod tests {
    use super::*;

    fn control(tag: &str) -> String {
        format!(
            r#"<w:sdt><w:sdtPr><w:tag w:val="{tag}"/></w:sdtPr><w:sdtContent><w:p><w:r><w:t>{tag}</w:t></w:r></w:p></w:sdtContent></w:sdt>"#
        )
    }

    /// A body of a paragraph, a block control, a paragraph holding an inline
    /// control, and a table whose first cell holds a block control.
    fn document() -> Document {
        let body = format!(
            r#"<w:p><w:r><w:t>first</w:t></w:r></w:p>{}<w:p><w:r><w:t>before</w:t></w:r><w:sdt><w:sdtPr><w:tag w:val="inline"/></w:sdtPr><w:sdtContent><w:r><w:t>inline</w:t></w:r></w:sdtContent></w:sdt></w:p><w:tbl><w:tblPr/><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:tcPr/>{}<w:p/></w:tc></w:tr></w:tbl>"#,
            control("block"),
            control("cell")
        );
        let bytes = Document::new().to_bytes().expect("a new document saves");
        let mut package =
            OpcPackage::from_reader(std::io::Cursor::new(bytes)).expect("the package opens");
        let part = package.main_document_part().expect("a main document part");
        let xml = String::from_utf8(package.get_part(&part).expect("main part").to_vec())
            .expect("the main part is UTF-8");
        let start = xml.find("<w:body>").expect("a body") + "<w:body>".len();
        let end = xml.find("<w:sectPr").expect("section properties");
        package.set_part(
            &part,
            format!("{}{body}{}", &xml[..start], &xml[end..]).into_bytes(),
        );
        let mut out = std::io::Cursor::new(Vec::new());
        package.write_to(&mut out).expect("the package writes");
        Document::from_bytes(&out.into_inner()).expect("the document opens")
    }

    /// The location and the block place of the control tagged `tag`.
    fn placed(document: &Document, tag: &str) -> (ContentLocation, ControlBlock) {
        let needle = format!(r#"<w:tag w:val="{tag}"/>"#);
        for story in document.stories().unwrap() {
            for (index, item) in document.story_items(&story).unwrap().iter().enumerate() {
                if item.kind() != StoryItemKind::ContentControl
                    || !String::from_utf8_lossy(&item.xml().unwrap()).contains(&needle)
                {
                    continue;
                }
                let location =
                    ContentLocation::new(story.clone(), StoryItemKind::ContentControl, vec![index]);
                let block = match (tag, story.kind()) {
                    ("block", StoryKind::Body) => ControlBlock {
                        slot: 1,
                        owner_index: 0,
                        item_index: 0,
                    },
                    // The paragraph, then the control inside it.
                    ("inline", StoryKind::Body) => ControlBlock {
                        slot: 2,
                        owner_index: 0,
                        item_index: 1,
                    },
                    // The cell's properties, then the control.
                    ("cell", StoryKind::TableCell) => ControlBlock {
                        slot: 3,
                        owner_index: 0,
                        item_index: 1,
                    },
                    other => panic!("unexpected control {other:?}"),
                };
                return (location, block);
            }
        }
        panic!("no control tagged {tag}")
    }

    #[test]
    fn a_control_property_edit_in_its_block_saves_what_the_staged_edit_saves() {
        for tag in ["block", "inline", "cell"] {
            for edit in [
                ContentControlEdit::Tag(Some("renamed".into())),
                ContentControlEdit::Alias(Some("Alias".into())),
                ContentControlEdit::Lock(Some(ContentControlLock::SdtLocked)),
                ContentControlEdit::Appearance(Some(ContentControlAppearance::Hidden)),
                ContentControlEdit::Placeholder(Some(true)),
                ContentControlEdit::Type(ContentControlTypePayload::PlainText),
            ] {
                let mut live = document();
                let (location, block) = placed(&live, tag);
                let mut staged = live.clone_for_staging();
                staged
                    .edit_content_control(&location, edit.clone())
                    .expect("the staged edit");
                assert_eq!(
                    live.edit_content_control_in_block(&location, block, &edit)
                        .expect("the block edit"),
                    Some(true),
                    "{tag} {edit:?}"
                );
                assert_eq!(live.to_bytes().unwrap(), staged.to_bytes().unwrap());
                // The same edit again changes nothing.
                assert_eq!(
                    live.edit_content_control_in_block(&location, block, &edit)
                        .expect("the block edit"),
                    Some(false),
                    "{tag} {edit:?} again"
                );
            }
        }
    }

    #[test]
    fn a_refused_control_edit_in_its_block_leaves_the_document_as_it_was() {
        let mut live = document();
        let (location, block) = placed(&live, "block");
        let unchanged = live.clone_for_staging().to_bytes().unwrap();
        let refused = [
            ContentControlEdit::Tag(Some("bad\u{1}".into())),
            ContentControlEdit::Type(ContentControlTypePayload::RepeatingSectionItem),
            ContentControlEdit::Binding {
                store_item_id: "{00000000-0000-0000-0000-000000000000}".into(),
                xpath: "/data/v".into(),
                prefix_mappings: None,
            },
        ];
        for edit in refused {
            assert!(
                live.edit_content_control_in_block(&location, block, &edit)
                    .is_err(),
                "{edit:?}"
            );
            assert_eq!(live.clone_for_staging().to_bytes().unwrap(), unchanged);
        }
    }

    #[test]
    fn a_block_that_does_not_place_the_control_leaves_the_edit_to_the_staged_route() {
        let mut live = document();
        let (location, mut block) = placed(&live, "block");
        block.slot = 0;
        let unchanged = live.clone_for_staging().to_bytes().unwrap();
        assert_eq!(
            live.edit_content_control_in_block(
                &location,
                block,
                &ContentControlEdit::Tag(Some("renamed".into()))
            )
            .unwrap(),
            None
        );
        assert_eq!(live.clone_for_staging().to_bytes().unwrap(), unchanged);
    }
}

//! Bullet and numbered lists: adding items at levels 0 to 8, restarting a
//! count, list definitions and instances (add, read, update, remove), number
//! formats and level templates, styles linked to a list level, the numbering
//! graph check, and the list markers a reader sees, on new documents and on
//! the lists Word, Google Docs, LibreOffice and python-docx saved.

use std::io::Cursor;

use oxoffice_docx::{
    Alignment, Document, ListLevel, ListLevelRestart, ListLevelSuffix, ListNumberFormat,
    NumberingLevelOverride, StyleBuilder, Twips,
};
use oxoffice_opc::OpcPackage;

use crate::support::{corpus_doc, doc_with, reopen, texts};

/// The marker a reader sees before each body paragraph, as layout draws it,
/// in paragraph order: "" for a paragraph that is not a list item.
fn markers(doc: &Document) -> Vec<String> {
    let layout = doc.layout_deterministic().expect("document lays out");
    (0..doc.paragraph_count())
        .map(|index| {
            doc.content_index_of_paragraph(index)
                .and_then(|body| layout.document_body_paragraph_numbering(body))
                .map(|numbering| numbering.marker_text.clone())
                .unwrap_or_default()
        })
        .collect()
}

/// A new document with one list: a definition of these levels and one
/// instance of it. Returns the instance id, which paragraphs take.
fn single_list(levels: &[ListLevel]) -> (Document, u32) {
    let mut doc = Document::new();
    let definition = doc
        .add_numbering_definition(levels)
        .expect("levels are valid");
    let id = doc
        .add_numbering_instance(definition, &[])
        .expect("definition exists");
    (doc, id)
}

/// The definition an instance uses.
fn definition_of(doc: &Document, id: u32) -> u32 {
    doc.numbering_instance(id)
        .expect("instance exists")
        .definition_id
}

/// Append one paragraph per level, on list `id`, named "item 1", "item 2"...
fn add_items(doc: &mut Document, id: u32, levels: &[u32]) {
    for (index, level) in levels.iter().enumerate() {
        doc.add_paragraph(&format!("item {}", index + 1))
            .numbering(id, *level);
    }
}

/// A document whose Heading1 and Heading2 styles are linked to levels 0 and
/// 1 of one list numbered "1", "1.1", with a plain paragraph in the middle.
fn numbered_headings() -> (Document, u32) {
    let (mut doc, id) = single_list(&[
        ListLevel::decimal().level_text("%1"),
        ListLevel::decimal().level_text("%1.%2"),
    ]);
    doc.link_style_to_numbering("Heading1", id, 0).unwrap();
    doc.link_style_to_numbering("Heading2", id, 1).unwrap();
    for (text, style) in [
        ("Introduction", "Heading1"),
        ("Scope", "Heading2"),
        ("Terms", "Heading2"),
        ("Body text", "Normal"),
        ("Method", "Heading1"),
        ("Data", "Heading2"),
    ] {
        doc.add_paragraph(text).style(style);
    }
    (doc, id)
}

// Adding items.

#[test]
fn list_levels_zero_to_eight_are_accepted_and_nine_is_refused() {
    for numbered in [false, true] {
        let mut doc = Document::new();
        for level in 0..9 {
            if numbered {
                doc.try_add_numbered_list_item("item", level).unwrap();
            } else {
                doc.try_add_bullet_list_item("item", level).unwrap();
            }
        }
        let before = doc.to_bytes().unwrap();
        let refused = if numbered {
            doc.try_add_numbered_list_item("too deep", 9).map(|_| ())
        } else {
            doc.try_add_bullet_list_item("too deep", 9).map(|_| ())
        };
        let error = refused.unwrap_err().to_string();
        assert!(error.contains("between 0 and 8"), "{error}");
        let id = doc.paragraph(0).unwrap().numbering().unwrap().0;
        assert!(!doc.paragraph_mut(0).unwrap().set_numbering(id, 9));
        assert_eq!(doc.to_bytes().unwrap(), before, "a refusal changes nothing");

        let doc = reopen(&mut doc);
        let levels: Vec<_> = doc
            .paragraphs()
            .iter()
            .map(|p| p.numbering().unwrap().1)
            .collect();
        assert_eq!(levels, (0..9).collect::<Vec<_>>());
        assert_eq!(doc.numbering_definitions().len(), 1);
        assert_eq!(doc.numbering_instances().len(), 1);
        doc.validate_numbering_graph().unwrap();
    }
}

#[test]
fn list_items_keep_cjk_rtl_emoji_and_empty_text() {
    let mut doc = Document::new();
    doc.add_bullet_list_item("第一项", 0);
    doc.add_bullet_list_item("البند الأول", 0);
    doc.add_numbered_list_item("😀 emoji", 0);
    doc.add_numbered_list_item("", 0);

    let doc = reopen(&mut doc);
    assert_eq!(texts(&doc), ["第一项", "البند الأول", "😀 emoji", ""]);
    // An empty item is still a list item with its marker.
    assert_eq!(markers(&doc), ["•", "•", "1.", "2."]);
}

// Restarting a count.

#[test]
fn restarting_while_adding_starts_a_new_count_that_later_items_follow() {
    let mut doc = Document::new();
    doc.add_numbered_list_item("one", 0);
    doc.add_numbered_list_item("two", 0);
    doc.try_add_numbered_list_item_with_restart("again one", 0, true)
        .unwrap();
    doc.add_numbered_list_item("again two", 0);
    let numbering: Vec<_> = doc.paragraphs().iter().map(|p| p.numbering()).collect();
    assert_ne!(numbering[2], numbering[1]);
    assert_eq!(numbering[3], numbering[2]);
    assert_eq!(markers(&doc), ["1.", "2.", "1.", "2."]);

    // The next session goes on with the restarted count.
    let mut doc = reopen(&mut doc);
    doc.add_numbered_list_item("again three", 0);
    assert_eq!(markers(&doc), ["1.", "2.", "1.", "2.", "3."]);
}

#[test]
fn restarting_an_item_numbered_through_its_style_moves_the_rest_of_its_list() {
    // Word's List Number items take their numbering from the style alone.
    let mut doc = corpus_doc("word/report.docx");
    let hire = texts(&doc)
        .iter()
        .position(|text| text.starts_with("Hire two"))
        .unwrap();
    let style_numbered = doc.paragraph(hire).unwrap().numbering();
    assert_eq!(style_numbered, None, "no direct numbering");
    assert_eq!(&markers(&doc)[hire..hire + 3], ["1.", "2.", "3."]);

    let new = doc.restart_numbering(hire + 1, 1).unwrap();
    let mut doc = reopen(&mut doc);
    assert_eq!(&markers(&doc)[hire..hire + 3], ["1.", "1.", "2."]);
    assert_eq!(doc.paragraph(hire).unwrap().numbering(), None);
    for item in [hire + 1, hire + 2] {
        let paragraph = doc.paragraph(item).unwrap();
        assert_eq!(paragraph.numbering(), Some((new, 0)), "item {item}");
        assert_eq!(paragraph.style_id(), Some("Listenumros"), "item {item}");
    }

    // A numbered item added later goes on with the restarted count.
    doc.add_numbered_list_item("Hire a third", 0);
    let last = doc.paragraph_count() - 1;
    assert_eq!(doc.paragraph(last).unwrap().numbering(), Some((new, 0)));
    assert_eq!(markers(&doc)[last], "3.");
}

#[test]
fn restarting_an_item_moves_every_later_item_of_its_list_at_every_level() {
    let (mut doc, id) = single_list(&[
        ListLevel::decimal().level_text("%1."),
        ListLevel::decimal().level_text("%1.%2."),
    ]);
    let other = doc
        .add_numbering_instance(definition_of(&doc, id), &[])
        .unwrap();
    for (text, level) in [("one", 0), ("one a", 1), ("two", 0), ("two a", 1)] {
        doc.add_paragraph(text).numbering(id, level);
    }
    doc.add_table(1, 1)
        .cell(0, 0)
        .unwrap()
        .add_paragraph("in a table")
        .numbering(id, 0);
    for (text, list) in [("in a control", id), ("three", id), ("another list", other)] {
        doc.add_paragraph(text).numbering(list, 0);
    }
    // "in a control" as Word wraps a selection in a rich text control.
    let mut package = OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap())).unwrap();
    let main = package.main_document_part().unwrap();
    let xml = String::from_utf8(package.get_part(&main).unwrap().to_vec()).unwrap();
    let text = xml.find(">in a control<").unwrap();
    let start = xml[..text].rfind("<w:p>").unwrap();
    let end = text + xml[text..].find("</w:p>").unwrap() + "</w:p>".len();
    let xml = format!(
        "{}<w:sdt><w:sdtPr><w:id w:val=\"5\"/></w:sdtPr><w:sdtContent>{}</w:sdtContent></w:sdt>{}",
        &xml[..start],
        &xml[start..end],
        &xml[end..]
    );
    package.set_part(&main, xml.into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let mut doc = Document::from_bytes(&out.into_inner()).unwrap();
    assert_eq!(
        texts(&doc),
        [
            "one",
            "one a",
            "two",
            "two a",
            "in a control",
            "three",
            "another list"
        ]
    );

    let new = doc.restart_numbering(2, 1).unwrap();
    let doc = reopen(&mut doc);
    let numbering: Vec<_> = (0..doc.paragraph_count())
        .map(|index| doc.paragraph(index).unwrap().numbering())
        .collect();
    assert_eq!(
        numbering,
        [
            Some((id, 0)),
            Some((id, 1)),
            Some((new, 0)),
            Some((new, 1)),
            Some((new, 0)),
            Some((new, 0)),
            Some((other, 0)),
        ]
    );
    let in_table = doc
        .table(0)
        .unwrap()
        .cell(0, 0)
        .unwrap()
        .paragraphs()
        .find(|paragraph| paragraph.text() == "in a table")
        .unwrap()
        .numbering();
    assert_eq!(in_table, Some((new, 0)));
    assert_eq!(markers(&doc)[..4], ["1.", "1.1.", "1.", "1.1."]);
    assert_eq!(markers(&doc)[5], "4.");
}

#[test]
fn restarting_a_sub_item_moves_only_the_rest_of_its_sublist() {
    // The instance restarts level 0, as Word's own restarted lists and
    // `add_numbered_list_item(restart=True)` write it.
    let mut doc = Document::new();
    let definition = doc
        .add_numbering_definition(&[
            ListLevel::decimal().level_text("%1."),
            ListLevel::decimal().level_text("%2."),
        ])
        .unwrap();
    let id = doc
        .add_numbering_instance(definition, &[NumberingLevelOverride::new(0).start(1)])
        .unwrap();
    for (text, level) in [
        ("one", 0),
        ("one a", 1),
        ("one b", 1),
        ("one c", 1),
        ("two", 0),
        ("two a", 1),
        ("two b", 1),
    ] {
        doc.add_paragraph(text).numbering(id, level);
    }
    assert_eq!(markers(&doc), ["1.", "1.", "2.", "3.", "2.", "1.", "2."]);

    let new = doc.restart_numbering(2, 5).unwrap();
    let doc = reopen(&mut doc);
    assert_eq!(markers(&doc), ["1.", "1.", "5.", "6.", "2.", "1.", "2."]);
    let numbering: Vec<_> = (0..doc.paragraph_count())
        .map(|index| doc.paragraph(index).unwrap().numbering())
        .collect();
    assert_eq!(
        numbering,
        [
            Some((id, 0)),
            Some((id, 1)),
            Some((new, 1)),
            Some((new, 1)),
            Some((id, 0)),
            Some((id, 1)),
            Some((id, 1)),
        ]
    );
}

#[test]
fn restart_numbering_refuses_bullets_plain_paragraphs_and_missing_ones() {
    let mut doc = Document::new();
    doc.add_bullet_list_item("bullet", 0);
    doc.add_paragraph("plain");
    let before = doc.to_bytes().unwrap();
    for (paragraph, message) in [
        (0, "numbered non-bullet level"),
        (1, "is not numbered"),
        (5, "direct body paragraph"),
    ] {
        let error = doc.restart_numbering(paragraph, 1).unwrap_err().to_string();
        assert!(error.contains(message), "paragraph {paragraph}: {error}");
    }
    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn restart_numbering_refuses_an_instance_with_data_it_would_lose() {
    // A durable id names its instance and is not copied; any other unknown
    // attribute would be lost by the copy, so the restart is refused.
    let mut doc = Document::new();
    doc.add_numbered_list_item("step", 0);
    let mut package =
        oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(doc.to_bytes().unwrap()))
            .expect("package opens");
    let xml = String::from_utf8(package.get_part("/word/numbering.xml").unwrap().to_vec()).unwrap();
    // A durableId in another namespace is not Word's: it is refused too.
    for attribute in [r#"x:tag="kept""#, r#"x:durableId="7""#] {
        let xml = xml.replacen(
            "<w:num w:numId=\"1\"",
            &format!("<w:num xmlns:x=\"urn:example\" {attribute} w:numId=\"1\""),
            1,
        );
        assert!(xml.contains(attribute), "the instance carries {attribute}");
        package.set_part("/word/numbering.xml", xml.into_bytes());
        let mut bytes = std::io::Cursor::new(Vec::new());
        package.write_to(&mut bytes).unwrap();
        let mut doc = Document::from_bytes(&bytes.into_inner()).unwrap();

        let error = doc.restart_numbering(0, 1).unwrap_err().to_string();
        assert!(
            error.contains("cannot project unsupported override source"),
            "{attribute}: {error}"
        );
    }
}

// List definitions and instances.

#[test]
fn number_formats_show_the_markers_word_shows_and_keep_their_names() {
    use ListNumberFormat::*;
    for (format, expected) in [
        (Decimal, ["1.", "2.", "3.", "4."]),
        (UpperLetter, ["A.", "B.", "C.", "D."]),
        (LowerLetter, ["a.", "b.", "c.", "d."]),
        (UpperRoman, ["I.", "II.", "III.", "IV."]),
        (LowerRoman, ["i.", "ii.", "iii.", "iv."]),
        (Chicago, ["*.", "\u{2020}.", "\u{2021}.", "\u{a7}."]),
        (Bullet, ["•", "•", "•", "•"]),
    ] {
        let (mut doc, id) = single_list(&[ListLevel::new(format.clone())]);
        add_items(&mut doc, id, &[0, 0, 0, 0]);
        let doc = reopen(&mut doc);
        assert_eq!(markers(&doc), expected, "{format:?}");
        assert_eq!(doc.numbering_is_bullet(id), Some(format == Bullet));
    }

    // Formats the layout draws no marker for still keep their exact name.
    assert_eq!(ListNumberFormat::from_name("lowerLetter"), LowerLetter);
    assert_eq!(
        ListNumberFormat::from_name("Decimal"),
        Other("Decimal".to_owned()),
        "format names are case-sensitive"
    );
    for name in ["decimalZero", "ordinal", "cardinalText"] {
        let (mut doc, id) = single_list(&[ListLevel::new(ListNumberFormat::from_name(name))]);
        add_items(&mut doc, id, &[0]);
        let doc = reopen(&mut doc);
        assert_eq!(doc.numbering_level(id, 0).unwrap().format_name, name);
        assert_eq!(doc.numbering_is_bullet(id), Some(false));
    }
}

#[test]
fn level_text_combines_parent_numbers_and_legal_numbering_turns_them_decimal() {
    let (mut doc, id) = single_list(&[
        ListLevel::new(ListNumberFormat::UpperRoman).level_text("%1."),
        ListLevel::decimal()
            .level_text("%1.%2.")
            .legal_numbering(true),
        ListLevel::decimal().level_text("%1.%2.%3"),
    ]);
    add_items(&mut doc, id, &[0, 1, 1, 2, 2, 0, 1, 2]);

    let doc = reopen(&mut doc);
    // Level 1 is legal, so its parent shows as "1" there, and stays "I" at
    // level 2, which is not.
    assert_eq!(
        markers(&doc),
        [
            "I.", "1.1.", "1.2.", "I.2.1", "I.2.2", "II.", "2.1.", "II.1.1"
        ]
    );
    assert_eq!(
        doc.numbering_level(id, 1).unwrap().legal_numbering,
        Some(true)
    );
}

#[test]
fn a_nested_level_restarts_under_each_parent_unless_told_never_to() {
    let lettered = ListLevel::new(ListNumberFormat::LowerLetter);
    for (nested, expected) in [
        (lettered.clone(), ["1.", "a.", "b.", "2.", "a."]),
        (
            lettered.restart(ListLevelRestart::Never),
            ["1.", "a.", "b.", "2.", "c."],
        ),
    ] {
        let (mut doc, id) = single_list(&[ListLevel::decimal(), nested]);
        add_items(&mut doc, id, &[0, 1, 1, 0, 1]);
        let doc = reopen(&mut doc);
        assert_eq!(markers(&doc), expected);
    }
}

#[test]
fn updating_a_definition_or_an_instance_changes_the_markers_of_its_lists() {
    let (mut doc, first) = single_list(&[ListLevel::decimal().start(3)]);
    let definition = definition_of(&doc, first);
    let second = doc
        .add_numbering_instance(definition, &[NumberingLevelOverride::new(0).start(10)])
        .unwrap();
    add_items(&mut doc, first, &[0, 0]);
    add_items(&mut doc, second, &[0, 0]);

    doc.update_numbering_instance(
        second,
        definition,
        &[NumberingLevelOverride::new(0).start(20)],
    )
    .unwrap();
    // Edit the level as read: the format and template change, the start of
    // 3 stays.
    let mut levels = doc.numbering_definition(definition).unwrap().levels;
    levels[0].properties.format = ListNumberFormat::UpperRoman;
    levels[0].properties = levels[0].properties.clone().level_text("(%1)");
    doc.update_numbering_definition(definition, &levels)
        .unwrap();

    let doc = reopen(&mut doc);
    assert_eq!(markers(&doc), ["(III)", "(IV)", "(XX)", "(XXI)"]);
    let level = &doc.numbering_definition(definition).unwrap().levels[0].properties;
    assert_eq!(level.format, ListNumberFormat::UpperRoman);
    assert_eq!(level.level_text_value(), Some("(%1)"));
    assert_eq!(level.start, Some(3));
    doc.validate_numbering_graph().unwrap();
}

#[test]
fn only_a_list_nothing_uses_can_be_removed() {
    let (mut doc, id) = single_list(&[ListLevel::decimal()]);
    let definition = definition_of(&doc, id);
    doc.add_paragraph("item").numbering(id, 0);

    let before = doc.to_bytes().unwrap();
    let error = doc.remove_numbering_instance(id).unwrap_err().to_string();
    assert!(error.contains("referenced by document content"), "{error}");
    let error = doc
        .remove_numbering_definition(definition)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains(&format!("referenced by instance {id}")),
        "{error}"
    );
    assert_eq!(doc.to_bytes().unwrap(), before);

    doc.paragraph_mut(0).unwrap().set_numbering_value(None);
    assert!(doc.remove_numbering_instance(id).unwrap());
    assert!(doc.remove_numbering_definition(definition).unwrap());
    assert!(!doc.remove_numbering_instance(id).unwrap(), "already gone");

    let doc = reopen(&mut doc);
    assert!(doc.numbering_instances().is_empty());
    assert!(doc.numbering_definitions().is_empty());
    assert_eq!(texts(&doc), ["item"]);
    doc.validate_numbering_graph().unwrap();
}

#[test]
fn numbering_readers_answer_nothing_for_unknown_ids() {
    let fresh = Document::new();
    assert!(fresh.numbering_definitions().is_empty());
    assert!(fresh.numbering_instances().is_empty());
    fresh.validate_numbering_graph().unwrap();

    let (mut doc, id) = single_list(&[ListLevel::decimal()]);
    assert_eq!(doc.numbering_is_bullet(99), None);
    assert!(doc.numbering_level(0, 0).is_none(), "0 means no list");
    assert!(doc.numbering_level(id, 1).is_none(), "a one-level list");
    assert!(doc.numbering_instance(99).is_none());
    assert!(doc.numbering_definition(99).is_none());
    assert!(!doc.set_list_level(99, 0, ListLevel::bullet()));
    assert!(!doc.remove_numbering_instance(99).unwrap());
    assert!(!doc.remove_numbering_definition(99).unwrap());
}

#[test]
fn level_settings_read_back_after_a_save() {
    let (mut doc, id) = single_list(&[ListLevel::new(ListNumberFormat::UpperLetter)
        .start(4)
        .level_text("%1)")
        .suffix(ListLevelSuffix::Space)
        .alignment(Alignment::Right)
        .indentation(Some(Twips(1080)), Some(Twips(540)), None)]);
    add_items(&mut doc, id, &[0, 0]);

    let doc = reopen(&mut doc);
    assert_eq!(markers(&doc), ["D)", "E)"]);
    let level = doc.numbering_level(id, 0).unwrap();
    assert_eq!(level.format_name, "upperLetter");
    assert_eq!(level.start, 4);
    assert_eq!(level.level_text, Some("%1)"));
    assert_eq!(level.suffix, ListLevelSuffix::Space);
    assert_eq!(level.alignment, Some(Alignment::Right));
    assert_eq!(level.indent_left, Some(Twips(1080)));
    assert_eq!(level.indent_hanging, Some(Twips(540)));
    let definition = doc.numbering_definition(definition_of(&doc, id)).unwrap();
    let properties = &definition.levels[0].properties;
    assert_eq!(properties.suffix_value(), Some(ListLevelSuffix::Space));
    assert_eq!(
        properties.indentation_value(),
        (Some(Twips(1080)), Some(Twips(540)), None)
    );
}

#[test]
fn invalid_list_levels_are_refused_without_a_change() {
    let mut doc = Document::new();
    let before = doc.to_bytes().unwrap();
    let both_indents = ListLevel::decimal().indentation(None, Some(Twips(360)), Some(Twips(360)));
    for (levels, message) in [
        (vec![], "require 1 through 9 levels"),
        (vec![ListLevel::decimal(); 10], "require 1 through 9 levels"),
        (
            vec![ListLevel::decimal().level_text("%2.")],
            "invalid placeholder %2",
        ),
        (vec![both_indents], "both hanging and first-line"),
        (
            vec![ListLevel::decimal().restart(ListLevelRestart::After(0))],
            "cannot restart after level 0",
        ),
        (
            vec![ListLevel::decimal().template_code("xyz")],
            "eight hexadecimal digits",
        ),
    ] {
        let error = doc
            .add_numbering_definition(&levels)
            .unwrap_err()
            .to_string();
        assert!(error.contains(message), "{error}");
    }
    // The older helper answers 0 instead of an error.
    assert_eq!(
        doc.add_list_definition(&[ListLevel::decimal().restart(ListLevelRestart::After(0))]),
        0
    );
    let error = doc.add_numbering_instance(77, &[]).unwrap_err().to_string();
    assert!(error.contains("definition 77 does not exist"), "{error}");
    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn set_list_level_redefines_a_deeper_level_after_the_fact() {
    let mut doc = Document::new();
    let id = doc.add_list_definition(&[ListLevel::bullet()]);
    assert!(doc.set_list_level(id, 1, ListLevel::decimal().start(3)));
    assert!(
        !doc.set_list_level(id, 9, ListLevel::decimal()),
        "no level 9"
    );
    assert!(
        !doc.set_list_level(
            id,
            1,
            ListLevel::decimal().restart(ListLevelRestart::After(1))
        ),
        "an invalid level"
    );
    add_items(&mut doc, id, &[0, 1, 1]);

    let doc = reopen(&mut doc);
    assert_eq!(markers(&doc), ["•", "3.", "4."]);
    let level = doc.numbering_level(id, 1).unwrap();
    assert_eq!((level.format_name, level.start), ("decimal", 3));
}

// List text as a reader sees it.

// Styles linked to a list level.

#[test]
fn an_instance_update_keeps_a_style_link_held_by_a_swapped_in_level() {
    let (mut doc, numbered) = single_list(&[ListLevel::decimal()]);
    let swapped = doc
        .add_numbering_instance(
            definition_of(&doc, numbered),
            &[NumberingLevelOverride::new(0).replacement(ListLevel::bullet())],
        )
        .unwrap();
    doc.add_style(StyleBuilder::paragraph("Points", "Points"))
        .unwrap();
    doc.link_style_to_numbering("Points", swapped, 0).unwrap();
    let link = doc.numbering_instance(swapped).unwrap().level_overrides[0]
        .paragraph_style_link
        .clone();
    assert_eq!(
        link.as_deref(),
        Some("Points"),
        "the swapped-in level holds the link"
    );

    let before = doc.to_bytes().unwrap();
    let error = doc
        .update_numbering_instance(swapped, definition_of(&doc, swapped), &[])
        .unwrap_err()
        .to_string();
    assert!(error.contains("style-link"), "{error}");
    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn a_level_that_names_a_style_not_numbered_by_it_is_refused() {
    // The other way round from LibreOffice's one-way link: the level names
    // Heading2, whose style no longer points at any list.
    let (mut doc, _) = numbered_headings();
    let mut package =
        oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(doc.to_bytes().unwrap()))
            .expect("package opens");
    let xml = String::from_utf8(package.get_part("/word/styles.xml").unwrap().to_vec()).unwrap();
    let style = xml
        .find(r#"w:styleId="Heading2""#)
        .expect("the Heading2 style");
    let start = style + xml[style..].find("<w:numPr>").expect("its numbering");
    let end = start + xml[start..].find("</w:numPr>").unwrap() + "</w:numPr>".len();
    let xml = format!("{}{}", &xml[..start], &xml[end..]);
    package.set_part("/word/styles.xml", xml.into_bytes());
    let mut bytes = std::io::Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    let doc = Document::from_bytes(&bytes.into_inner()).unwrap();

    let error = doc.validate_numbering_graph().unwrap_err().to_string();
    assert!(
        error.contains("and style 'Heading2' are not reciprocally linked"),
        "{error}"
    );
}

#[test]
fn unlinking_a_style_stops_numbering_its_paragraphs() {
    let (mut doc, id) = numbered_headings();
    let mut doc = reopen(&mut doc);
    doc.unlink_style_from_numbering("Heading2", id, 1).unwrap();
    let error = doc
        .unlink_style_from_numbering("Heading1", id, 1)
        .unwrap_err()
        .to_string();
    assert!(error.contains("are not linked"), "{error}");

    let doc = reopen(&mut doc);
    assert_eq!(markers(&doc), ["1", "", "", "", "2", ""]);
    let properties = doc
        .style("Heading2")
        .unwrap()
        .paragraph_properties()
        .cloned();
    assert_eq!(
        properties.map(|p| (p.num_id, p.num_ilvl)),
        Some((None, None))
    );
    assert_eq!(doc.numbering_level(id, 1).unwrap().paragraph_style, None);
    doc.validate_numbering_graph().unwrap();
}

#[test]
fn linking_a_style_refuses_conflicts_and_styles_that_cannot_hold_a_list() {
    let (mut doc, id) = numbered_headings();
    doc.add_style(StyleBuilder::character("Emphasis", "Emphasis"))
        .unwrap();
    let before = doc.to_bytes().unwrap();
    for (style, level, instance, message) in [
        (
            "Heading1",
            1,
            id,
            "already references a different numbering level",
        ),
        ("Heading3", 0, id, "already linked to another style"),
        ("Emphasis", 0, id, "is not a paragraph style"),
        ("Missing", 0, id, "does not exist"),
        ("Heading3", 0, 99, "numbering instance 99 does not exist"),
    ] {
        let error = doc
            .link_style_to_numbering(style, instance, level)
            .unwrap_err()
            .to_string();
        assert!(error.contains(message), "{style} {level}: {error}");
    }
    assert_eq!(doc.to_bytes().unwrap(), before);
}

/// `doc` saved with `edit` applied to its styles part, opened again.
fn with_styles_xml(doc: &mut Document, edit: impl FnOnce(String) -> String) -> Document {
    let mut package = OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap())).unwrap();
    let xml = String::from_utf8(package.get_part("/word/styles.xml").unwrap().to_vec()).unwrap();
    package.set_part("/word/styles.xml", edit(xml).into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    Document::from_bytes(&out.into_inner()).unwrap()
}

/// The style's own `(w:numId, w:ilvl)`.
fn style_numbering(doc: &Document, style: &str) -> (Option<u32>, Option<u32>) {
    doc.style(style)
        .unwrap()
        .paragraph_properties()
        .map_or((None, None), |p| (p.num_id, p.num_ilvl))
}

#[test]
fn a_style_numbered_without_a_level_links_at_the_level_its_based_on_chain_gives() {
    // Child holds only a `w:numId`; the `w:ilvl 2` it shows comes from Deep,
    // through Mid, which holds no numbering.
    let (mut doc, id) = single_list(&[
        ListLevel::decimal().level_text("%1."),
        ListLevel::decimal().level_text("%1.%2."),
        ListLevel::decimal().level_text("%1.%2.%3."),
    ]);
    let mut doc = with_styles_xml(&mut doc, |xml| {
        xml.replace(
            "</w:styles>",
            &format!(
                concat!(
                    r#"<w:style w:type="paragraph" w:styleId="Deep"><w:name w:val="Deep"/>"#,
                    r#"<w:pPr><w:numPr><w:ilvl w:val="2"/></w:numPr></w:pPr></w:style>"#,
                    r#"<w:style w:type="paragraph" w:styleId="Mid"><w:name w:val="Mid"/>"#,
                    r#"<w:basedOn w:val="Deep"/></w:style>"#,
                    r#"<w:style w:type="paragraph" w:styleId="Child"><w:name w:val="Child"/>"#,
                    r#"<w:basedOn w:val="Mid"/><w:pPr><w:numPr><w:numId w:val="{id}"/>"#,
                    r#"</w:numPr></w:pPr></w:style></w:styles>"#
                ),
                id = id
            ),
        )
    });
    doc.add_paragraph("item").style("Child");
    assert_eq!(markers(&doc), ["1.1.1."], "Child shows at level 2");

    // A new `w:basedOn` would move its paragraphs to level 0: only the link
    // calls change a style's numbering.
    let before = doc.to_bytes().unwrap();
    let error = doc
        .set_style(StyleBuilder::paragraph("Child", "Child").based_on("Normal"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("numbering must be changed"), "{error}");
    let error = doc
        .link_style_to_numbering("Child", id, 0)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("already references a different numbering level"),
        "{error}"
    );
    assert_eq!(doc.to_bytes().unwrap(), before);

    doc.link_style_to_numbering("Child", id, 2).unwrap();
    let mut doc = reopen(&mut doc);
    doc.validate_numbering_graph().unwrap();
    assert_eq!(
        style_numbering(&doc, "Child"),
        (Some(id), None),
        "its form is kept"
    );
    assert_eq!(
        doc.numbering_level(id, 2).unwrap().paragraph_style,
        Some("Child")
    );
    assert_eq!(markers(&doc), ["1.1.1."]);

    // A new `w:basedOn` higher up the chain would move Child too.
    let before = doc.to_bytes().unwrap();
    let error = doc
        .set_style(StyleBuilder::paragraph("Mid", "Mid").based_on("Normal"))
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("style 'Child' numbering must be changed"),
        "{error}"
    );
    assert_eq!(doc.to_bytes().unwrap(), before);
    // An instance update that would drop the level's name of Child is refused.
    let plain = doc
        .add_numbering_definition(&[
            ListLevel::decimal(),
            ListLevel::decimal(),
            ListLevel::decimal(),
        ])
        .unwrap();
    let after_definition = doc.to_bytes().unwrap();
    let error = doc
        .update_numbering_instance(id, plain, &[])
        .unwrap_err()
        .to_string();
    assert!(error.contains("style-link"), "{error}");
    assert_eq!(doc.to_bytes().unwrap(), after_definition);

    // Removing Child, once nothing uses it, releases the level that names it.
    let mut unused = Document::from_bytes(&doc.to_bytes().unwrap()).unwrap();
    let item = unused.content_index_of_paragraph(0).unwrap();
    assert!(unused.remove_content(item));
    assert!(unused.remove_style("Child").unwrap());
    unused.validate_numbering_graph().unwrap();
    assert_eq!(unused.numbering_level(id, 2).unwrap().paragraph_style, None);

    doc.unlink_style_from_numbering("Child", id, 2).unwrap();
    let doc = reopen(&mut doc);
    doc.validate_numbering_graph().unwrap();
    assert_eq!(style_numbering(&doc, "Child"), (None, None));
    assert_eq!(doc.numbering_level(id, 2).unwrap().paragraph_style, None);
}

#[test]
fn a_style_numbered_without_a_level_reads_at_level_zero_when_a_level_names_it() {
    // Heading2 holds only a `w:numId` and level 1 names it back. Word,
    // LibreOffice and layout draw it at level 0: the name does not set the
    // level, so the facade reads level 0 too, and the graph check finds the
    // link one way only.
    let (mut doc, id) = numbered_headings();
    let doc = with_styles_xml(&mut doc, |xml| {
        let style = xml.find(r#"w:styleId="Heading2""#).unwrap();
        let level = style + xml[style..].find(r#"<w:ilvl w:val="1"/>"#).unwrap();
        format!(
            "{}{}",
            &xml[..level],
            &xml[level + r#"<w:ilvl w:val="1"/>"#.len()..]
        )
    });
    assert_eq!(style_numbering(&doc, "Heading2"), (Some(id), None));
    assert_eq!(markers(&doc), ["1", "2", "3", "", "4", "5"]);
    let scope = doc.paragraph(1).unwrap();
    assert_eq!(
        doc.effective_paragraph_properties(&scope)
            .num_ilvl
            .unwrap_or(0),
        0
    );
    let error = doc.validate_numbering_graph().unwrap_err().to_string();
    assert!(error.contains("are not reciprocally linked"), "{error}");
}

// The numbering graph.

#[test]
fn a_new_list_skips_an_identifier_named_inside_raw_content() {
    // An element of another vocabulary holds a paragraph on list 1, which the
    // document does not define: the body keeps it as raw XML.
    let mut package =
        OpcPackage::from_reader(Cursor::new(doc_with(&["first"]).to_bytes().unwrap()))
            .expect("package opens");
    let xml = String::from_utf8(package.get_part("/word/document.xml").unwrap().to_vec()).unwrap();
    let raw = concat!(
        r#"<x:box xmlns:x="urn:example:x"><w:p><w:pPr><w:numPr><w:ilvl w:val="0"/>"#,
        r#"<w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>boxed</w:t></w:r></w:p></x:box>"#
    );
    let at = xml
        .find("<w:sectPr")
        .expect("the body ends with its section");
    let xml = format!("{}{raw}{}", &xml[..at], &xml[at..]);
    package.set_part("/word/document.xml", xml.into_bytes());
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    let mut doc = Document::from_bytes(&bytes.into_inner()).unwrap();

    doc.try_add_bullet_list_item("item", 0).unwrap();
    let item = doc.paragraph_count() - 1;
    assert_ne!(doc.paragraph(item).unwrap().numbering().unwrap().0, 1);
    let saved = doc.to_bytes().unwrap();
    let xml = OpcPackage::from_reader(Cursor::new(saved))
        .unwrap()
        .get_part("/word/document.xml")
        .map(|bytes| String::from_utf8(bytes.to_vec()).unwrap())
        .unwrap();
    assert_eq!(xml.matches(r#"<w:numId w:val="1"/>"#).count(), 1, "{xml}");
}

#[test]
fn a_list_item_with_no_relationship_id_left_for_its_numbering_part_is_refused_without_a_change() {
    // The main part's relationships hold the last numeric id, so the
    // numbering part, reserved first, can get no relationship.
    let mut package =
        OpcPackage::from_reader(Cursor::new(doc_with(&["first"]).to_bytes().unwrap())).unwrap();
    package
        .get_or_create_part_rels("/word/document.xml")
        .add_with_id(
            "rId4294967295",
            "http://example.com/relationships/probe",
            "document.xml",
        );
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    let mut doc = Document::from_bytes(&bytes.into_inner()).unwrap();
    let before = doc.to_bytes().unwrap();

    for _ in 0..2 {
        let error = doc
            .try_add_bullet_list_item("item", 0)
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("numbering relationship allocation failed"),
            "{error}"
        );
    }
    assert_eq!(doc.to_bytes().unwrap(), before);
    assert!(doc.numbering_instances().is_empty());
}

#[test]
fn list_items_are_refused_on_a_body_a_typed_edit_cannot_leave_savable() {
    // The w:body element declares a namespace of its own, as some producers
    // write it: the body saves as it was opened, and a typed edit of it
    // cannot be saved, so the typed inserts refuse at the call.
    let mut package =
        OpcPackage::from_reader(Cursor::new(doc_with(&["first"]).to_bytes().unwrap()))
            .expect("package opens");
    let xml = String::from_utf8(package.get_part("/word/document.xml").unwrap().to_vec()).unwrap();
    let xml = xml.replacen("<w:body>", r#"<w:body xmlns:x="urn:example:x">"#, 1);
    package.set_part("/word/document.xml", xml.into_bytes());
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    let mut doc = Document::from_bytes(&bytes.into_inner()).unwrap();
    let before = doc.to_bytes().unwrap();

    for numbered in [false, true, false] {
        let result = if numbered {
            doc.try_add_numbered_list_item("item", 0).map(|_| ())
        } else {
            doc.try_add_bullet_list_item("item", 0).map(|_| ())
        };
        let error = result.unwrap_err().to_string();
        assert!(error.contains("shadowed `x` namespace"), "{error}");
    }
    assert_eq!(doc.to_bytes().unwrap(), before);
    assert!(doc.numbering_instances().is_empty());
}

#[test]
fn an_addition_to_an_instance_on_a_missing_definition_is_refused_without_a_change() {
    // Instance 1 names definition 5, which the numbering part lacks, and a
    // paragraph is numbered by instance 1.
    let (mut doc, id) = single_list(&[ListLevel::decimal()]);
    doc.add_paragraph("item").numbering(id, 0);
    let mut package =
        OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap())).expect("package opens");
    let xml = String::from_utf8(package.get_part("/word/numbering.xml").unwrap().to_vec()).unwrap();
    let definition = definition_of(&doc, id);
    let xml = xml.replacen(
        &format!(r#"<w:abstractNumId w:val="{definition}"/>"#),
        r#"<w:abstractNumId w:val="5"/>"#,
        1,
    );
    package.set_part("/word/numbering.xml", xml.into_bytes());
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    let mut doc = Document::from_bytes(&bytes.into_inner()).unwrap();
    let before = doc.to_bytes().unwrap();

    let error = doc
        .add_numbering_definition(&[ListLevel::decimal()])
        .unwrap_err()
        .to_string();
    assert!(error.contains("references missing definition 5"), "{error}");
    assert_eq!(doc.to_bytes().unwrap(), before);
}

#[test]
fn content_on_the_last_identifier_does_not_exhaust_the_lists_identifiers() {
    let (mut doc, id) = single_list(&[ListLevel::decimal()]);
    doc.add_paragraph("orphan").numbering(u32::MAX, 0);
    let added = doc
        .add_numbering_instance(definition_of(&doc, id), &[])
        .unwrap();
    assert!(added != id && added != u32::MAX, "{added}");
    doc.add_paragraph("item").numbering(added, 0);

    let doc = reopen(&mut doc);
    assert_eq!(doc.paragraph(0).unwrap().numbering(), Some((u32::MAX, 0)));
    assert_eq!(markers(&doc), ["", "1."]);
}

#[test]
fn an_edit_that_would_leave_content_on_a_missing_list_is_refused_without_a_change() {
    let (mut doc, id) = single_list(&[ListLevel::decimal(), ListLevel::decimal()]);
    add_items(&mut doc, id, &[0, 1]);
    let shallow = doc
        .add_numbering_definition(&[ListLevel::decimal()])
        .unwrap();
    let (mut bullets, bullet) = single_list(&[ListLevel::bullet()]);
    let before = doc.to_bytes().unwrap();
    let bullets_before = bullets.to_bytes().unwrap();

    // Another definition without the level an item uses.
    let error = doc
        .update_numbering_instance(id, shallow, &[])
        .unwrap_err()
        .to_string();
    assert!(
        error.contains(&format!("numbering instance {id} at missing level 1")),
        "{error}"
    );
    // A list item at a level the list it joins does not have.
    let error = doc
        .try_add_numbered_list_item("deeper", 4)
        .map(|_| ())
        .unwrap_err()
        .to_string();
    assert!(
        error.contains(&format!("numbering instance {id} at missing level 4")),
        "{error}"
    );
    let error = bullets
        .try_add_bullet_list_item("deeper", 4)
        .map(|_| ())
        .unwrap_err()
        .to_string();
    assert!(
        error.contains(&format!("numbering instance {bullet} at missing level 4")),
        "{error}"
    );
    assert_eq!(bullets.to_bytes().unwrap(), bullets_before);
    assert_eq!(doc.to_bytes().unwrap(), before);
}

// Lists of the corpus reports.

/// The list element (`<ul>` or `<ol>`) that holds the HTML item showing
/// `text`: the last list tag opened before it.
fn html_list_of<'a>(html: &'a str, text: &str) -> &'a str {
    let at = html
        .find(text)
        .unwrap_or_else(|| panic!("no HTML holds {text:?}"));
    let before = &html[..at];
    let ul = before.rfind("<ul");
    let ol = before.rfind("<ol");
    let closed = before.rfind("</ul>").max(before.rfind("</ol>"));
    match ul.max(ol) {
        Some(open) if Some(open) > closed => {
            if ul == Some(open) {
                "ul"
            } else {
                "ol"
            }
        }
        _ => "none",
    }
}

#[test]
fn exports_show_the_report_lists_of_every_producer() {
    // Word, LibreOffice and python-docx number these paragraphs through
    // their List Bullet and List Number styles, Google Docs on the paragraphs.
    for name in [
        "word/report.docx",
        "google/report.docx",
        "libreoffice/report.docx",
        "authored/report.docx",
    ] {
        let doc = corpus_doc(name);
        let markdown = doc.to_markdown();
        for item in [
            "- Subscription revenue reached 4.2 million euros, up from 3.7 million.",
            "- The Lyon warehouse opened on schedule and serves the southern region.",
            "1. Hire two support engineers for the evening shift.",
        ] {
            assert!(
                markdown.lines().any(|line| line == item),
                "{name}: Markdown shows {item:?}:\n{markdown}"
            );
        }
        // RTF keeps them too, read back as lists.
        let rtf = doc.to_rtf_bytes().unwrap();
        let reread = Document::from_rtf_bytes(&rtf.bytes).unwrap().document;
        let reread_markdown = reread.to_markdown();
        for item in [
            "- Subscription revenue reached 4.2 million euros, up from 3.7 million.",
            "1. Hire two support engineers for the evening shift.",
        ] {
            assert!(
                reread_markdown.lines().any(|line| line == item),
                "{name}: RTF keeps {item:?}:\n{reread_markdown}"
            );
        }
        let html = doc.to_html_fragment();
        assert_eq!(
            html_list_of(&html, "Subscription revenue reached"),
            "ul",
            "{name}"
        );
        assert_eq!(
            html_list_of(&html, "Hire two support engineers"),
            "ol",
            "{name}"
        );
    }
}

#[test]
fn exports_keep_headings_numbered_through_their_style_as_headings() {
    // Word numbers its headings by linking the heading styles to a list.
    let (doc, _) = numbered_headings();
    let markdown = doc.to_markdown();
    for line in ["# Introduction", "## Scope", "# Method"] {
        assert!(
            markdown.lines().any(|candidate| candidate == line),
            "Markdown keeps {line:?}:\n{markdown}"
        );
    }
    let html = doc.to_html_fragment();
    assert!(html.contains("<h1>Introduction</h1>"), "{html}");
    assert!(html.contains("<h2>Scope</h2>"), "{html}");
    assert!(!html.contains("<ol>"), "{html}");
}

#[test]
fn exports_keep_a_direct_level_on_the_list_of_its_style() {
    // Word writes a sub-clause as its style's list with a direct w:ilvl and
    // no w:numId: the level is not dropped.
    let (mut doc, id) = single_list(&[ListLevel::decimal(), ListLevel::decimal()]);
    doc.add_style(StyleBuilder::paragraph("Clauses", "Clauses"))
        .unwrap();
    doc.link_style_to_numbering("Clauses", id, 0).unwrap();
    doc.add_paragraph("clause").style("Clauses");
    doc.add_paragraph("sub clause").style("Clauses");
    let mut package =
        OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap())).expect("package opens");
    let xml = String::from_utf8(package.get_part("/word/document.xml").unwrap().to_vec()).unwrap();
    let style = r#"<w:pStyle w:val="Clauses"/>"#;
    let second = xml.rfind(style).expect("the sub clause's style");
    let xml = format!(
        "{}{style}<w:numPr><w:ilvl w:val=\"1\"/></w:numPr>{}",
        &xml[..second],
        &xml[second + style.len()..]
    );
    package.set_part("/word/document.xml", xml.into_bytes());
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    let doc = Document::from_bytes(&bytes.into_inner()).unwrap();

    let rtf = doc.to_rtf_bytes().unwrap();
    let text = String::from_utf8(rtf.bytes.clone()).unwrap();
    assert!(text.contains(r"\ilvl1"), "{text}");
    let odt = doc.to_odt_bytes().unwrap();
    let messages: Vec<&str> = rtf
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .chain(
            odt.diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.as_str()),
        )
        .collect();
    assert!(
        !messages
            .iter()
            .any(|message| message.contains("without a numbering id")
                || message.contains("without numbering id")),
        "{messages:?}"
    );
}

#[test]
fn rtf_reads_a_font_named_with_unicode_characters() {
    // LibreOffice's "ＭＳ ゴシック" comes as \u characters between bytes.
    let rtf = br"{\rtf1\ansi{\fonttbl{\f0 A\u-211?B\u-205? C;}}\f0 x\par}";
    let doc = Document::from_rtf_bytes(rtf).unwrap().document;
    assert_eq!(
        doc.paragraph(0).unwrap().run(0).unwrap().font_name(),
        Some("AＭBＳ C")
    );
}

#[test]
fn exports_keep_a_list_open_around_its_unmarked_sub_paragraphs() {
    // Contracts number their clauses and leave the sub-paragraphs unmarked
    // on a `none` level: layout keeps counting the clauses around them.
    let (mut doc, id) =
        single_list(&[ListLevel::decimal(), ListLevel::new(ListNumberFormat::None)]);
    add_items(&mut doc, id, &[0, 1, 0, 1, 0]);
    assert_eq!(markers(&doc), ["1.", "", "2.", "", "3."]);

    let markdown = doc.to_markdown();
    let lines: Vec<&str> = markdown.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(
        lines,
        [
            "1. item 1",
            "   item 2",
            "2. item 3",
            "   item 4",
            "3. item 5"
        ],
        "one list, each sub-paragraph under its clause:\n{markdown}"
    );
    let html = doc.to_html_fragment();
    assert_eq!(html.matches("<ol>").count(), 1, "{html}");
    assert!(html.contains("<li>item 1<p>item 2</p></li>"), "{html}");

    // ODT writes the `none` level as a numbered level with no number, so
    // LibreOffice draws nothing on the sub-paragraphs and keeps counting.
    let odt = doc.to_odt_bytes().unwrap();
    let messages: Vec<&str> = odt
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();
    assert!(
        !messages.iter().any(|message| message.contains("decimal")),
        "a none level is not exported as a decimal level: {messages:?}"
    );
    let reread = Document::from_odt_bytes(&odt.bytes).unwrap().document;
    assert_eq!(markers(&reread), ["1.", "", "2.", "", "3."]);
}

#[test]
fn exports_keep_counting_a_clause_whose_body_text_follows_its_sub_items() {
    // A clause, its sub-item (a), then body text of the clause on a `none`
    // level shallower than the sub-item, then the next clause.
    let (mut doc, id) = single_list(&[
        ListLevel::decimal(),
        ListLevel::new(ListNumberFormat::None),
        ListLevel::new(ListNumberFormat::LowerLetter).level_text("%3."),
    ]);
    add_items(&mut doc, id, &[0, 2, 1, 0]);
    assert_eq!(markers(&doc), ["1.", "a.", "", "2."]);

    let markdown = doc.to_markdown();
    assert!(
        !markdown.lines().any(|line| line == "item 3"),
        "the body text stays in the list:\n{markdown}"
    );
    let html = doc.to_html_fragment();
    // One outer list from the first clause to the second.
    let outer = html.find("<ol>").unwrap();
    let second = html.find("item 4").unwrap();
    assert!(
        !html[outer..second].contains("</ol>\n<p>item 3</p>\n<ol>"),
        "the outer list does not close around the body text: {html}"
    );
    assert_eq!(
        html.matches("</ol>").count(),
        html.matches("<ol").count(),
        "{html}"
    );
    // The body text sits in the clause's item, after its sub-list, so an
    // HTML reader keeps it.
    assert!(
        html.contains("<li>item 2</li>\n</ol>\n<p>item 3</p></li>"),
        "{html}"
    );
    let reread = Document::from_html(&doc.to_html()).unwrap().document;
    assert_eq!(texts(&reread), ["item 1", "item 2", "item 3", "item 4"]);
    assert!(
        html[..second].rfind("<ol>").unwrap() < html.find("item 3").unwrap(),
        "no list opens again before the second clause: {html}"
    );
}

/// The document after `edit` changed the XML of one of its parts, saved and
/// reopened: markup Word writes that the facade does not.
fn with_part(doc: &mut Document, part: &str, edit: impl Fn(String) -> String) -> Document {
    let bytes = doc.to_bytes().expect("document saves");
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).expect("package opens");
    let xml = String::from_utf8(package.get_part(part).expect("part exists").to_vec())
        .expect("part is UTF-8");
    package.set_part(part, edit(xml).into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).expect("package writes");
    Document::from_bytes(&out.into_inner()).expect("edited document opens")
}

/// What each Markdown list item shows, in order: its number, or "-" for a
/// bullet.
fn markdown_item_markers(markdown: &str) -> Vec<String> {
    markdown
        .lines()
        .filter_map(|line| {
            let line = line.trim_start();
            if line.starts_with("- ") {
                return Some("-".to_owned());
            }
            let digits: String = line.chars().take_while(char::is_ascii_digit).collect();
            (!digits.is_empty() && line[digits.len()..].starts_with(". ")).then_some(digits)
        })
        .collect()
}

/// What a browser shows before each HTML list item, in order: the number it
/// counts from its `<ol start>`, or "-" in a `<ul>`.
fn html_item_numbers(html: &str) -> Vec<String> {
    let mut lists: Vec<Option<u32>> = Vec::new();
    let mut numbers = Vec::new();
    let mut rest = html;
    while let Some(at) = rest.find('<') {
        rest = &rest[at..];
        let tag = &rest[..rest.find('>').unwrap() + 1];
        if tag.starts_with("<ol") {
            let start = tag.split("start=\"").nth(1).map_or(1, |value| {
                value[..value.find('"').unwrap()].parse().unwrap()
            });
            lists.push(Some(start));
        } else if tag.starts_with("<ul") {
            lists.push(None);
        } else if tag == "</ol>" || tag == "</ul>" {
            lists.pop();
        } else if tag.starts_with("<li") {
            match lists.last_mut().expect("an item is in a list") {
                Some(next) => {
                    numbers.push(next.to_string());
                    *next += 1;
                }
                None => numbers.push("-".to_owned()),
            }
        }
        rest = &rest[tag.len()..];
    }
    numbers
}

#[test]
fn exports_start_a_new_list_after_items_of_a_skipped_level() {
    // Levels 0, 2, 1: the level-1 item is not the next item of the level-2
    // list, though Markdown writes both at the same column.
    let (mut doc, id) = single_list(&[
        ListLevel::decimal(),
        ListLevel::new(ListNumberFormat::LowerLetter).level_text("%2."),
        ListLevel::new(ListNumberFormat::LowerRoman).level_text("%3."),
    ]);
    add_items(&mut doc, id, &[0, 2, 1]);
    assert_eq!(markers(&doc), ["1.", "i.", "a."]);
    let markdown = doc.to_markdown();
    assert_eq!(
        markdown, "1. item 1\n   1. item 2\n\n   <!-- -->\n\n   1. item 3\n",
        "{markdown}"
    );
    let html = doc.to_html_fragment();
    assert_eq!(html_item_numbers(&html), ["1", "1", "1"], "{html}");

    // A list that starts at level 1, then a level-0 item.
    let (mut doc, id) = single_list(&[
        ListLevel::decimal(),
        ListLevel::new(ListNumberFormat::LowerLetter).level_text("%2."),
    ]);
    add_items(&mut doc, id, &[1, 1, 0]);
    assert_eq!(markers(&doc), ["a.", "b.", "1."]);
    let markdown = doc.to_markdown();
    assert_eq!(
        markdown, "1. item 1\n2. item 2\n\n<!-- -->\n\n1. item 3\n",
        "{markdown}"
    );
    let html = doc.to_html_fragment();
    assert_eq!(html_item_numbers(&html), ["1", "2", "1"], "{html}");
}

#[test]
fn exports_write_a_heading_whose_numbering_is_turned_off_after_the_list() {
    let (mut doc, id) = single_list(&[ListLevel::decimal()]);
    add_items(&mut doc, id, &[0]);
    doc.add_paragraph("Heading")
        .style("Heading2")
        .numbering(0, 0);
    let markdown = doc.to_markdown();
    assert!(markdown.contains("\n\n## Heading\n"), "{markdown}");
    assert!(!markdown.contains("   ## Heading"), "{markdown}");
    let html = doc.to_html_fragment();
    assert!(html.contains("</ol>\n<h2>Heading</h2>"), "{html}");
}

#[test]
fn exports_part_two_numbered_lists_that_count_on_their_own() {
    let (mut doc, first) = single_list(&[ListLevel::decimal()]);
    let second = doc
        .add_numbering_definition(&[ListLevel::decimal()])
        .unwrap();
    let second = doc.add_numbering_instance(second, &[]).unwrap();
    doc.add_paragraph("a1").numbering(first, 0);
    doc.add_paragraph("a2").numbering(first, 0);
    doc.add_paragraph("b1").numbering(second, 0);
    assert_eq!(markers(&doc), ["1.", "2.", "1."]);

    let markdown = doc.to_markdown();
    assert_eq!(
        markdown, "1. a1\n2. a2\n\n<!-- -->\n\n1. b1\n",
        "{markdown}"
    );
    let html = doc.to_html_fragment();
    assert_eq!(html.matches("<ol>").count(), 2, "{html}");
    assert_eq!(html_item_numbers(&html), ["1", "2", "1"], "{html}");
}

#[test]
fn exports_count_the_list_items_of_a_table_as_layout_does() {
    let (mut doc, id) = single_list(&[ListLevel::decimal()]);
    add_items(&mut doc, id, &[0]);
    {
        let mut table = doc.add_table(2, 1);
        for row in 0..2 {
            let mut cell = table.cell(row, 0).unwrap();
            cell.add_paragraph("in a cell").numbering(id, 0);
        }
    }
    doc.add_paragraph("after the table").numbering(id, 0);
    assert_eq!(markers(&doc), ["1.", "4."]);

    let markdown = doc.to_markdown();
    assert!(
        markdown.lines().any(|line| line == "4. after the table"),
        "{markdown}"
    );
    let html = doc.to_html_fragment();
    assert!(
        html.contains("<ol start=\"4\">\n<li>after the table"),
        "{html}"
    );

    // Layout leaves out a row deleted as a tracked change, the other kept.
    let doc = with_part(&mut doc, "/word/document.xml", |xml| {
        let at = xml.find("<w:tr").unwrap();
        let at = at + xml[at..].find('>').unwrap() + 1;
        format!(
            r#"{}<w:trPr><w:del w:id="90" w:author="A" w:date="2026-01-01T00:00:00Z"/></w:trPr>{}"#,
            &xml[..at],
            &xml[at..]
        )
    });
    assert_eq!(markers(&doc), ["1.", "3."]);
    let markdown = doc.to_markdown();
    assert!(
        markdown.lines().any(|line| line == "3. after the table"),
        "{markdown}"
    );
    let html = doc.to_html_fragment();
    assert!(
        html.contains("<ol start=\"3\">\n<li>after the table"),
        "{html}"
    );
}

#[test]
fn exports_restart_a_list_after_a_section_break_as_layout_does() {
    // A definition marked to restart after a section break, as Word 2013
    // writes it, and a section break between its items.
    let (mut doc, id) = single_list(&[ListLevel::decimal()]);
    add_items(&mut doc, id, &[0]);
    doc.add_paragraph("end of the section");
    add_items(&mut doc, id, &[0]);
    let mut doc = with_part(&mut doc, "/word/numbering.xml", |xml| {
        xml.replacen(
            "<w:numbering ",
            r#"<w:numbering xmlns:w15="http://schemas.microsoft.com/office/word/2012/wordml" "#,
            1,
        )
        .replacen(
            "<w:abstractNum ",
            r#"<w:abstractNum w15:restartNumberingAfterBreak="1" "#,
            1,
        )
    });
    let doc = with_part(&mut doc, "/word/document.xml", |xml| {
        let text = xml.find("end of the section").unwrap();
        let at = xml[..text].rfind("<w:p>").expect("a plain paragraph") + "<w:p>".len();
        format!(
            concat!(
                "{}<w:pPr><w:sectPr><w:pgSz w:w=\"12240\" w:h=\"15840\"/>",
                "<w:pgMar w:top=\"1440\" w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\" ",
                "w:header=\"720\" w:footer=\"720\" w:gutter=\"0\"/></w:sectPr></w:pPr>{}"
            ),
            &xml[..at],
            &xml[at..]
        )
    });
    assert_eq!(doc.section_count(), 2);
    assert_eq!(markers(&doc), ["1.", "", "1."]);
    let markdown = doc.to_markdown();
    assert_eq!(markdown_item_markers(&markdown), ["1", "1"], "{markdown}");
    let html = doc.to_html_fragment();
    assert_eq!(html_item_numbers(&html), ["1", "1"], "{html}");
}

#[test]
fn exports_show_the_level_an_instance_swaps_in() {
    let (mut doc, numbered) = single_list(&[ListLevel::decimal()]);
    let dashed = doc
        .add_numbering_instance(
            definition_of(&doc, numbered),
            &[NumberingLevelOverride::new(0).replacement(ListLevel::bullet().level_text("–"))],
        )
        .unwrap();
    add_items(&mut doc, dashed, &[0]);
    assert_eq!(markers(&doc), ["–"]);

    assert!(doc.to_markdown().lines().any(|line| line == "- item 1"));
    assert_eq!(html_list_of(&doc.to_html_fragment(), "item 1"), "ul");
    let odt = doc.to_odt_bytes().unwrap();
    let reread = Document::from_odt_bytes(&odt.bytes).unwrap().document;
    let (id, _) = reread
        .paragraph(0)
        .unwrap()
        .numbering()
        .expect("a list item");
    assert_eq!(
        reread.numbering_is_bullet(id),
        Some(true),
        "the ODT list is a bullet list"
    );
}

#[test]
fn a_paragraph_without_a_level_restarts_at_the_level_its_style_gives() {
    let mut doc = Document::new();
    for text in ["a", "b", "c"] {
        doc.add_numbered_list_item(text, 1);
    }
    let id = doc.paragraph(0).unwrap().numbering().unwrap().0;
    // A heading style numbered at level 1, and items that name it with
    // their own w:numId and no w:ilvl: they show at level 1.
    let mut package = OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap())).unwrap();
    let main = package.main_document_part().unwrap();
    let xml = String::from_utf8(package.get_part(&main).unwrap().to_vec()).unwrap();
    assert!(xml.contains(r#"<w:ilvl w:val="1"/>"#), "{xml}");
    let xml = xml
        .replace(r#"<w:ilvl w:val="1"/>"#, "")
        .replace("<w:pPr>", r#"<w:pPr><w:pStyle w:val="Level2"/>"#);
    package.set_part(&main, xml.into_bytes());
    let styles_part = main.replace("document.xml", "styles.xml");
    let styles = String::from_utf8(package.get_part(&styles_part).unwrap().to_vec()).unwrap();
    let styles = styles.replace(
        "</w:styles>",
        &format!(
            r#"<w:style w:type="paragraph" w:styleId="Level2"><w:name w:val="Level 2"/><w:pPr><w:numPr><w:ilvl w:val="1"/><w:numId w:val="{id}"/></w:numPr></w:pPr></w:style></w:styles>"#
        ),
    );
    package.set_part(&styles_part, styles.into_bytes());
    // Level 1 counts in decimal, so that an added item may continue it.
    let numbering_part = main.replace("document.xml", "numbering.xml");
    let numbering = String::from_utf8(package.get_part(&numbering_part).unwrap().to_vec()).unwrap();
    let level = numbering.find(r#"w:ilvl="1""#).unwrap();
    let format = level + numbering[level..].find("<w:numFmt w:val=\"").unwrap();
    let end = format + numbering[format..].find("/>").unwrap() + 2;
    let numbering = format!(
        r#"{}<w:numFmt w:val="decimal"/>{}"#,
        &numbering[..format],
        &numbering[end..]
    );
    package.set_part(&numbering_part, numbering.into_bytes());
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let mut doc = Document::from_bytes(&out.into_inner()).unwrap();
    let new = doc.restart_numbering(2, 5).unwrap();
    let mut doc = reopen(&mut doc);
    assert_eq!(doc.paragraph(2).unwrap().numbering(), Some((new, 1)));
    // The restarted item as a producer writes it, without its w:ilvl: an
    // item added at level 1 still continues the restarted list.
    let mut package = OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap())).unwrap();
    let xml = String::from_utf8(package.get_part(&main).unwrap().to_vec()).unwrap();
    package.set_part(
        &main,
        xml.replace(r#"<w:ilvl w:val="1"/>"#, "").into_bytes(),
    );
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let mut doc = Document::from_bytes(&out.into_inner()).unwrap();
    doc.add_numbered_list_item("d", 1);
    let doc = reopen(&mut doc);
    assert_eq!(doc.paragraph(3).unwrap().numbering().unwrap().0, new);
}

#[test]
fn a_paragraph_numbered_without_a_level_takes_the_indentation_of_level_zero() {
    let mut doc = Document::new();
    doc.add_numbered_list_item("a", 0);
    let numbered = doc.paragraph(0).unwrap();
    let expected = doc.effective_paragraph_properties(&numbered).ind_left;
    assert!(expected.is_some(), "level 0 of a new list is indented");
    // The same item as a producer writes it: a w:numId and no w:ilvl.
    let mut package = OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap())).unwrap();
    let main = package.main_document_part().unwrap();
    let xml = String::from_utf8(package.get_part(&main).unwrap().to_vec()).unwrap();
    assert!(xml.contains(r#"<w:ilvl w:val="0"/>"#), "{xml}");
    package.set_part(
        &main,
        xml.replace(r#"<w:ilvl w:val="0"/>"#, "").into_bytes(),
    );
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let doc = Document::from_bytes(&out.into_inner()).unwrap();
    let paragraph = doc.paragraph(0).unwrap();
    assert_eq!(
        doc.effective_paragraph_properties(&paragraph).ind_left,
        expected
    );
}

#[test]
fn a_level_text_the_file_holds_as_a_reference_xml_cannot_carry_still_lets_numbering_change() {
    // Another producer wrote a character XML 1.0 cannot carry as a character
    // reference in a level text. The file keeps it as read; only a level
    // text the caller supplies is checked.
    let mut doc = doc_with(&["item"]);
    let num_id = doc.add_list_definition(&[ListLevel::decimal().level_text("%1.")]);
    let mut package = OpcPackage::from_reader(Cursor::new(doc.to_bytes().unwrap())).unwrap();
    let xml = String::from_utf8(package.get_part("/word/numbering.xml").unwrap().to_vec()).unwrap();
    assert!(xml.contains(r#"w:val="%1.""#), "{xml}");
    package.set_part(
        "/word/numbering.xml",
        xml.replace(r#"w:val="%1.""#, r#"w:val="%1&#7;""#)
            .into_bytes(),
    );
    let mut out = Cursor::new(Vec::new());
    package.write_to(&mut out).unwrap();
    let mut doc = Document::from_bytes(&out.into_inner()).unwrap();

    doc.add_numbering_definition(&[ListLevel::decimal().level_text("%1)")])
        .expect("a new definition is accepted");
    assert!(
        doc.set_list_level(num_id, 0, ListLevel::decimal().level_text("%1\u{7}:")),
        "the infallible setter drops the character"
    );
    let doc = Document::from_bytes(&doc.to_bytes().expect("the document saves")).unwrap();
    let saved = doc.part_xml("word/numbering.xml").unwrap();
    assert!(saved.contains(r#"w:val="%1:""#), "{saved}");
}

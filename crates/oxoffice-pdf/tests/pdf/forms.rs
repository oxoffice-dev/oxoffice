//! Form field fragments become AcroForm fields with appearance streams.

use oxoffice_layout::{
    ChoiceOption, Color, DocumentStructure, FieldAlignment, FormField, FormFieldFragment,
    FormFieldId, FormFieldKind, GroupElement, LayoutResult, Point, PositionedElement, Rect,
    StructureId, StructureNode, StructureRole, Transform,
};
use oxoffice_pdf::{PdfConformance, PdfOptions, render_to_pdf_checked, render_to_pdf_with_options};

use crate::support::{PAGE_HEIGHT, Pdf, contains, layout, rect, run, text, text_origins};

pub(crate) fn field(name: &str, kind: FormFieldKind) -> FormField {
    FormField {
        name: name.to_owned(),
        tooltip: None,
        kind,
        read_only: false,
        font_size: 11.0,
        color: Color::BLACK,
        alignment: FieldAlignment::Left,
        placeholder: false,
    }
}

pub(crate) fn text_kind(value: &str) -> FormFieldKind {
    FormFieldKind::Text {
        value: value.to_owned(),
        multiline: false,
        max_len: None,
    }
}

/// One line's fragment: the value drawn on a baseline 12 points under the
/// top of `rect`.
pub(crate) fn fragment(field: u32, rect: Rect, value: &str) -> PositionedElement {
    PositionedElement::FormField(FormFieldFragment {
        field: FormFieldId(field),
        rect,
        children: vec![run(
            value,
            Point {
                x: rect.x,
                y: rect.y + 12.0,
            },
            11.0,
        )],
        alternate: Vec::new(),
        mark: None,
    })
}

pub(crate) fn line(x: f64, y: f64, width: f64) -> Rect {
    Rect {
        x,
        y,
        width,
        height: 14.0,
    }
}

pub(crate) fn label() -> PositionedElement {
    run("Name:", Point { x: 72.0, y: 112.0 }, 11.0)
}

pub(crate) fn with_fields(mut layout: LayoutResult, fields: Vec<FormField>) -> LayoutResult {
    layout.form_fields = fields;
    layout
}

/// Every fragment replaced by its children: the static drawing.
fn flattened(elements: &[PositionedElement]) -> Vec<PositionedElement> {
    elements
        .iter()
        .flat_map(|element| match element {
            PositionedElement::FormField(fragment) => flattened(&fragment.children),
            PositionedElement::Group(group) => vec![PositionedElement::Group(GroupElement {
                children: flattened(&group.children),
                ..group.clone()
            })],
            other => vec![other.clone()],
        })
        .collect()
}

pub(crate) fn pdf(layout: &LayoutResult) -> Pdf {
    Pdf::read(&render_to_pdf_checked(layout).unwrap())
}

#[test]
fn a_layout_without_fragments_writes_no_acroform() {
    let layout = layout(vec![vec![label()]]);
    let bytes = render_to_pdf_checked(&layout).unwrap();
    let read = Pdf::read(&bytes);
    assert!(read.catalog().get(b"AcroForm").is_err());
    assert!(read.page_widgets(0).is_empty());
    assert!(
        !contains(&bytes, b"WinAnsiEncoding"),
        "no typing font without fields"
    );
}

#[test]
fn fields_declared_but_never_drawn_write_no_acroform() {
    let layout = with_fields(
        layout(vec![vec![label()]]),
        vec![field("name", text_kind("Jane"))],
    );
    assert!(pdf(&layout).catalog().get(b"AcroForm").is_err());
}

#[test]
fn a_text_field_has_its_name_value_tooltip_flags_and_rectangle() {
    let mut name = field("name", text_kind("Jane"));
    name.tooltip = Some("Your full name".to_owned());
    name.read_only = true;
    name.alignment = FieldAlignment::Right;
    name.kind = FormFieldKind::Text {
        value: "Jane".to_owned(),
        multiline: true,
        max_len: Some(40),
    };
    let layout = with_fields(
        layout(vec![vec![
            label(),
            fragment(0, line(110.0, 100.0, 200.0), "Jane"),
        ]]),
        vec![name],
    );
    let read = pdf(&layout);
    let field = read.field("name");
    assert_eq!(field.get(b"FT").unwrap().as_name().unwrap(), b"Tx");
    assert_eq!(text(field.get(b"V").unwrap()), "Jane");
    assert_eq!(text(field.get(b"TU").unwrap()), "Your full name");
    let flags = field.get(b"Ff").unwrap().as_i64().unwrap();
    assert_eq!(flags & 1, 1, "read only");
    assert_eq!(flags & (1 << 12), 1 << 12, "multiline");
    assert_eq!(field.get(b"MaxLen").unwrap().as_i64().unwrap(), 40);
    assert_eq!(field.get(b"Q").unwrap().as_i64().unwrap(), 2);
    assert_eq!(
        rect(field.get(b"Rect").unwrap()),
        [
            110.0,
            (PAGE_HEIGHT - 114.0) as f32,
            310.0,
            (PAGE_HEIGHT - 100.0) as f32
        ]
    );
    let appearance = text(field.get(b"DA").unwrap());
    assert_eq!(appearance, "/LibSans 11 Tf 0 0 0 rg");
    assert_eq!(read.page_widgets(0).len(), 1);
}

#[test]
fn a_field_value_is_drawn_in_its_appearance_and_not_on_the_page() {
    let with_field = with_fields(
        layout(vec![vec![
            label(),
            fragment(0, line(110.0, 100.0, 200.0), "Jane"),
        ]]),
        vec![field("name", text_kind("Jane"))],
    );
    let without = layout(vec![vec![label()]]);
    let read = pdf(&with_field);
    assert_eq!(read.page_content(0), pdf(&without).page_content(0));
    let appearance = read.appearance(read.field("name"), None);
    assert_eq!(read.drawn_text(&appearance), "Jane");
}

#[test]
fn a_check_box_has_both_states_and_subsets_the_other_symbol() {
    let check = PositionedElement::FormField(FormFieldFragment {
        field: FormFieldId(0),
        rect: line(72.0, 100.0, 12.0),
        children: vec![run("X", Point { x: 72.0, y: 112.0 }, 11.0)],
        alternate: vec![run("O", Point { x: 72.0, y: 112.0 }, 11.0)],
        mark: None,
    });
    let layout = with_fields(
        layout(vec![vec![label(), check]]),
        vec![field("agree", FormFieldKind::CheckBox { checked: true })],
    );
    let read = pdf(&layout);
    let field = read.field("agree");
    assert_eq!(field.get(b"FT").unwrap().as_name().unwrap(), b"Btn");
    assert_eq!(field.get(b"V").unwrap().as_name().unwrap(), b"Yes");
    assert_eq!(field.get(b"AS").unwrap().as_name().unwrap(), b"Yes");
    assert_eq!(read.drawn_text(&read.appearance(field, Some(b"Yes"))), "X");
    assert_eq!(read.drawn_text(&read.appearance(field, Some(b"Off"))), "O");

    // The other state's glyph is in the embedded subset, mapped to its text.
    let page = read
        .doc
        .get_object(read.page_ids()[0])
        .unwrap()
        .as_dict()
        .unwrap();
    let fonts = read.dict(read.dict(page, b"Resources"), b"Font");
    let font = read
        .resolve(fonts.iter().next().unwrap().1)
        .as_dict()
        .unwrap();
    let to_unicode = read.stream(font.get(b"ToUnicode").unwrap());
    assert!(contains(&to_unicode, b"<004F>"), "O is mapped");
    assert!(contains(&to_unicode, b"<0058>"), "X is mapped");
}

#[test]
fn an_unchecked_box_shows_off_and_draws_its_current_symbol_there() {
    let check = PositionedElement::FormField(FormFieldFragment {
        field: FormFieldId(0),
        rect: line(72.0, 100.0, 12.0),
        children: vec![run("O", Point { x: 72.0, y: 112.0 }, 11.0)],
        alternate: vec![run("X", Point { x: 72.0, y: 112.0 }, 11.0)],
        mark: None,
    });
    let unchecked = with_fields(
        layout(vec![vec![check]]),
        vec![field("agree", FormFieldKind::CheckBox { checked: false })],
    );
    let read = pdf(&unchecked);
    let agree = read.field("agree");
    assert_eq!(agree.get(b"V").unwrap().as_name().unwrap(), b"Off");
    assert_eq!(agree.get(b"AS").unwrap().as_name().unwrap(), b"Off");

    // Unchecked, the drawing is the Off state and the other symbol is Yes.
    assert_eq!(read.drawn_text(&read.appearance(agree, Some(b"Off"))), "O");
    assert_eq!(read.drawn_text(&read.appearance(agree, Some(b"Yes"))), "X");
}

#[test]
fn a_choice_field_lists_its_options_and_holds_its_value() {
    let options = vec![
        ChoiceOption {
            value: "fr".to_owned(),
            display: "France".to_owned(),
        },
        ChoiceOption {
            value: "Spain".to_owned(),
            display: "Spain".to_owned(),
        },
    ];
    let list = field(
        "country",
        FormFieldKind::Choice {
            options: options.clone(),
            value: Some("fr".to_owned()),
            editable: false,
        },
    );
    let combo = field(
        "city",
        FormFieldKind::Choice {
            options,
            value: Some("Lyon".to_owned()),
            editable: true,
        },
    );
    let layout = with_fields(
        layout(vec![vec![
            fragment(0, line(72.0, 100.0, 150.0), "France"),
            fragment(1, line(72.0, 130.0, 150.0), "Lyon"),
        ]]),
        vec![list, combo],
    );
    let read = pdf(&layout);
    let country = read.field("country");
    assert_eq!(country.get(b"FT").unwrap().as_name().unwrap(), b"Ch");
    assert_eq!(text(country.get(b"V").unwrap()), "fr");
    let flags = country.get(b"Ff").unwrap().as_i64().unwrap();
    assert_eq!(flags & (1 << 17), 1 << 17, "combo");
    assert_eq!(flags & (1 << 18), 0, "not editable");
    let opt = country.get(b"Opt").unwrap().as_array().unwrap();
    let pair = opt[0].as_array().unwrap();
    assert_eq!(
        (text(&pair[0]).as_str(), text(&pair[1]).as_str()),
        ("fr", "France")
    );
    assert_eq!(
        text(&opt[1]),
        "Spain",
        "an option shown as its value is one string"
    );

    let city = read.field("city");
    assert_eq!(text(city.get(b"V").unwrap()), "Lyon");
    let flags = city.get(b"Ff").unwrap().as_i64().unwrap();
    assert_eq!(flags & (1 << 18), 1 << 18, "editable");
}

#[test]
fn a_placeholder_and_a_signature_show_nothing_and_leave_the_page() {
    let mut empty = field("name", text_kind(""));
    empty.placeholder = true;
    let layout_with = with_fields(
        layout(vec![vec![
            label(),
            fragment(0, line(110.0, 100.0, 200.0), "Click here"),
            fragment(1, line(110.0, 200.0, 200.0), "Sign here"),
        ]]),
        vec![empty, field("signature", FormFieldKind::Signature)],
    );
    let read = pdf(&layout_with);
    assert_eq!(
        read.page_content(0),
        pdf(&layout(vec![vec![label()]])).page_content(0)
    );

    let name = read.field("name");
    assert!(name.get(b"V").is_err(), "a placeholder is no value");
    assert!(read.appearance(name, None).is_empty());
    let signature = read.field("signature");
    assert_eq!(signature.get(b"FT").unwrap().as_name().unwrap(), b"Sig");
    assert!(signature.get(b"V").is_err());
    assert!(read.appearance(signature, None).is_empty());
}

#[test]
fn every_widget_is_printed_with_no_actions_and_no_need_appearances() {
    let layout = with_fields(
        layout(vec![vec![
            fragment(0, line(72.0, 100.0, 150.0), "Jane"),
            fragment(1, line(72.0, 130.0, 150.0), "Sign"),
        ]]),
        vec![
            field("name", text_kind("Jane")),
            field("signature", FormFieldKind::Signature),
        ],
    );
    let read = pdf(&layout);
    let form = read.dict(read.catalog(), b"AcroForm");
    assert!(form.get(b"NeedAppearances").is_err());
    for field in read.fields() {
        assert_eq!(field.get(b"Subtype").unwrap().as_name().unwrap(), b"Widget");
        assert_eq!(field.get(b"F").unwrap().as_i64().unwrap(), 4, "print only");
        assert!(field.get(b"A").is_err() && field.get(b"AA").is_err());
        let appearance = read.dict(field, b"AP");
        assert_eq!(appearance.len(), 1, "a normal appearance only");
        assert!(field.get(b"P").is_ok(), "the widget names its page");
    }
}

#[test]
fn the_typing_font_is_embedded_whole_with_win_ansi_widths() {
    let layout = with_fields(
        layout(vec![vec![fragment(0, line(72.0, 100.0, 150.0), "Jane")]]),
        vec![field("name", text_kind("Jane"))],
    );
    let read = pdf(&layout);
    let form = read.dict(read.catalog(), b"AcroForm");
    assert_eq!(text(form.get(b"DA").unwrap()), "/LibSans 0 Tf 0 g");
    let fonts = read.dict(read.dict(form, b"DR"), b"Font");
    let font = read.dict(fonts, b"LibSans");
    assert_eq!(
        font.get(b"Subtype").unwrap().as_name().unwrap(),
        b"TrueType"
    );
    assert_eq!(
        font.get(b"Encoding").unwrap().as_name().unwrap(),
        b"WinAnsiEncoding"
    );
    assert_eq!(font.get(b"FirstChar").unwrap().as_i64().unwrap(), 32);
    assert_eq!(font.get(b"LastChar").unwrap().as_i64().unwrap(), 255);
    let widths = font.get(b"Widths").unwrap().as_array().unwrap();
    assert_eq!(widths.len(), 224);
    assert!(widths[usize::from(b'A' - 32)].as_float().unwrap() > 500.0);
    assert!(
        widths[0x81 - 32].as_float().unwrap() == 0.0,
        "a WinAnsi hole has no glyph"
    );
    let descriptor = read.dict(font, b"FontDescriptor");
    let file = read.stream(descriptor.get(b"FontFile2").unwrap());
    assert_eq!(
        file,
        oxoffice_layout::bundled_fonts::LIBERATION_SANS_REGULAR
    );
}

#[test]
fn fragments_of_a_field_union_on_its_first_page_and_later_ones_stay_on_the_page() {
    let first = vec![
        fragment(0, line(300.0, 100.0, 200.0), "first line"),
        fragment(0, line(72.0, 114.0, 150.0), "second line"),
    ];
    let later = vec![fragment(0, line(72.0, 72.0, 100.0), "third line")];
    let layout = with_fields(
        layout(vec![first, later.clone()]),
        vec![field(
            "notes",
            text_kind("first line second line third line"),
        )],
    );
    let read = pdf(&layout);
    assert_eq!(read.page_widgets(0).len(), 1);
    assert!(read.page_widgets(1).is_empty());
    assert_eq!(
        rect(read.field("notes").get(b"Rect").unwrap()),
        [
            72.0,
            (PAGE_HEIGHT - 128.0) as f32,
            500.0,
            (PAGE_HEIGHT - 100.0) as f32
        ]
    );
    assert_eq!(read.drawn_text(&read.page_content(1)), "third line");
    assert_eq!(
        read.drawn_text(&read.page_content(0)),
        "",
        "page 1 keeps nothing"
    );
    let appearance = read.appearance(read.field("notes"), None);
    assert_eq!(read.drawn_text(&appearance), "first linesecond line");
    assert_operators_apart(&appearance);
}

#[test]
fn a_fragment_in_a_moved_group_places_its_widget_where_it_is_drawn() {
    let group = PositionedElement::Group(GroupElement {
        transform: Transform {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: 100.0,
            f: 50.0,
        },
        clip: None,
        opacity: 1.0,
        effects: Vec::new(),
        children: vec![fragment(0, line(10.0, 20.0, 80.0), "Jane")],
    });
    let layout = with_fields(
        layout(vec![vec![group]]),
        vec![field("name", text_kind("Jane"))],
    );
    assert_eq!(
        rect(pdf(&layout).field("name").get(b"Rect").unwrap()),
        [
            110.0,
            (PAGE_HEIGHT - 84.0) as f32,
            190.0,
            (PAGE_HEIGHT - 70.0) as f32
        ]
    );
}

#[test]
fn field_names_are_made_valid_and_unique_in_field_order() {
    let layout = with_fields(
        layout(vec![vec![
            fragment(0, line(72.0, 100.0, 100.0), "a"),
            fragment(1, line(72.0, 130.0, 100.0), "b"),
            fragment(2, line(72.0, 160.0, 100.0), "c"),
            fragment(3, line(72.0, 190.0, 100.0), "d"),
        ]]),
        vec![
            field("client.name", text_kind("a")),
            field("client_name", text_kind("b")),
            field("", text_kind("c")),
            field("client_name_2", text_kind("d")),
        ],
    );
    let names: Vec<String> = pdf(&layout)
        .fields()
        .iter()
        .map(|field| text(field.get(b"T").unwrap()))
        .collect();
    assert_eq!(
        names,
        ["client_name", "client_name_3", "field", "client_name_2"]
    );
}

#[test]
fn the_raster_draws_a_fragment_like_its_children() {
    let elements = vec![label(), fragment(0, line(110.0, 100.0, 200.0), "Jane")];
    let with_field = with_fields(
        layout(vec![elements.clone()]),
        vec![field("name", text_kind("Jane"))],
    );
    let static_layout = layout(vec![flattened(&elements)]);
    assert_eq!(
        oxoffice_pdf::render_page_to_png(&with_field, 0, 72.0),
        oxoffice_pdf::render_page_to_png(&static_layout, 0, 72.0)
    );
}

#[test]
fn form_output_is_identical_on_two_runs() {
    let layout = with_fields(
        layout(vec![vec![
            label(),
            fragment(0, line(110.0, 100.0, 200.0), "Jane"),
            fragment(1, line(110.0, 130.0, 200.0), "Sign"),
        ]]),
        vec![
            field("name", text_kind("Jane")),
            field("signature", FormFieldKind::Signature),
        ],
    );
    assert_eq!(
        render_to_pdf_checked(&layout).unwrap(),
        render_to_pdf_checked(&layout).unwrap()
    );
}

/// Whether a point lies inside a PDF rectangle, a hundredth of a point aside.
fn inside(point: (f64, f64), bbox: [f32; 4]) -> bool {
    let [x0, y0, x1, y1] = bbox.map(f64::from);
    (x0 - 0.01..=x1 + 0.01).contains(&point.0) && (y0 - 0.01..=y1 + 0.01).contains(&point.1)
}

fn close(a: (f64, f64), b: (f64, f64)) -> bool {
    (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01
}

#[test]
fn an_appearance_draws_its_value_where_the_static_export_draws_it() {
    let moved = PositionedElement::Group(GroupElement {
        transform: Transform {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: 100.0,
            f: 50.0,
        },
        clip: None,
        opacity: 1.0,
        effects: Vec::new(),
        children: vec![fragment(1, line(10.0, 220.0, 80.0), "Bob")],
    });
    let elements = vec![fragment(0, line(110.0, 100.0, 200.0), "Jane"), moved];
    let with_field = with_fields(
        layout(vec![elements.clone()]),
        vec![
            field("name", text_kind("Jane")),
            field("other", text_kind("Bob")),
        ],
    );
    let read = pdf(&with_field);
    let static_read = pdf(&layout(vec![flattened(&elements)]));
    let static_origins = text_origins(&static_read.page_content(0));
    assert_eq!(static_origins.len(), 2);

    for (name, static_origin) in ["name", "other"].into_iter().zip(static_origins) {
        let field = read.field(name);
        let bbox = read.appearance_bbox(field, None);
        assert_eq!(
            bbox,
            rect(field.get(b"Rect").unwrap()),
            "{name}: the box is the widget"
        );
        let origins = text_origins(&read.appearance(field, None));
        assert_eq!(origins.len(), 1);
        assert!(
            close(origins[0], static_origin),
            "{name}: {:?} vs {static_origin:?}",
            origins[0]
        );
        assert!(inside(origins[0], bbox), "{name}: drawn inside its box");
    }
    assert!(close(
        text_origins(&read.appearance(read.field("name"), None))[0],
        (110.0, 680.0)
    ));
}

#[test]
fn an_appearance_keeps_the_clip_and_opacity_of_the_groups_around_it() {
    let clipped = PositionedElement::Group(GroupElement {
        transform: Transform::IDENTITY,
        clip: Some(oxoffice_layout::Path::rect(Rect {
            x: 100.0,
            y: 90.0,
            width: 60.0,
            height: 30.0,
        })),
        opacity: 0.5,
        effects: Vec::new(),
        children: vec![fragment(0, line(110.0, 100.0, 200.0), "A long value")],
    });
    let layout = with_fields(
        layout(vec![vec![clipped]]),
        vec![field("name", text_kind("A long value"))],
    );
    let read = pdf(&layout);
    let appearance = read.appearance(read.field("name"), None);
    assert!(contains(&appearance, b"W\nn"), "clipped like the page");
    assert!(contains(&appearance, b" gs"), "faded like the page");
}

#[test]
fn a_check_box_draws_only_the_text_of_its_other_state() {
    let check = PositionedElement::FormField(FormFieldFragment {
        field: FormFieldId(0),
        rect: line(72.0, 100.0, 12.0),
        children: vec![run("X", Point { x: 72.0, y: 112.0 }, 11.0)],
        alternate: vec![
            PositionedElement::FilledRect {
                rect: line(72.0, 100.0, 12.0),
                color: Color::BLACK,
            },
            run("O", Point { x: 72.0, y: 112.0 }, 11.0),
        ],
        mark: None,
    });
    let layout = with_fields(
        layout(vec![vec![check]]),
        vec![field("agree", FormFieldKind::CheckBox { checked: true })],
    );
    let read = pdf(&layout);
    let off = read.appearance(read.field("agree"), Some(b"Off"));
    assert_eq!(read.drawn_text(&off), "O");
    assert!(!contains(&off, b" re"), "no rectangle drawn");
}

#[test]
fn a_tagged_fragment_drawn_on_the_page_is_an_artifact() {
    let mut layout = with_fields(
        layout(vec![
            vec![fragment(0, line(72.0, 100.0, 200.0), "first")],
            vec![PositionedElement::MarkedContent {
                structure: Some(StructureId::new(2).unwrap()),
                children: vec![fragment(0, line(72.0, 100.0, 200.0), "later")],
            }],
        ]),
        vec![field("notes", text_kind("first later"))],
    );
    layout.structure = Some(DocumentStructure {
        root: StructureId::new(1).unwrap(),
        nodes: vec![
            StructureNode {
                id: StructureId::new(1).unwrap(),
                role: StructureRole::Document,
                children: vec![StructureId::new(2).unwrap()],
                alternate_text: None,
            },
            StructureNode {
                id: StructureId::new(2).unwrap(),
                role: StructureRole::Paragraph,
                children: Vec::new(),
                alternate_text: None,
            },
        ],
    });
    let read = pdf(&layout);
    let later = read.page_content(1);
    assert_eq!(read.drawn_text(&later), "later");
    let artifact = position(&later, b"/Artifact BMC").expect("an artifact sequence");
    let text = position(&later, b"BT").expect("the text");
    let end = position(&later, b"EMC").expect("the end of the sequence");
    assert!(
        artifact < text && text < end,
        "the artifact surrounds the text"
    );
}

/// Where `needle` first sits in `haystack`.
fn position(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[test]
fn a_fragment_drawn_inside_a_tagged_paragraph_is_that_paragraph_content() {
    let layout = tagged(with_fields(
        layout(vec![
            vec![fragment(0, line(72.0, 100.0, 200.0), "first")],
            vec![paragraph(vec![
                run("Notes:", Point { x: 72.0, y: 112.0 }, 11.0),
                fragment(0, line(120.0, 100.0, 200.0), "later"),
            ])],
        ]),
        vec![field("notes", text_kind("first later"))],
    ));
    let read = pdf(&layout);
    let later = read.page_content(1);
    assert_eq!(read.drawn_text(&later), "Notes:later");
    assert!(
        !contains(&later, b"/Artifact"),
        "no artifact inside a marked sequence"
    );
}

pub(crate) fn structure_id(value: u32) -> StructureId {
    StructureId::new(value).unwrap()
}

/// A document element holding one paragraph element.
pub(crate) fn tagged(mut layout: LayoutResult) -> LayoutResult {
    layout.structure = Some(DocumentStructure {
        root: structure_id(1),
        nodes: vec![
            StructureNode {
                id: structure_id(1),
                role: StructureRole::Document,
                children: vec![structure_id(2)],
                alternate_text: None,
            },
            StructureNode {
                id: structure_id(2),
                role: StructureRole::Paragraph,
                children: Vec::new(),
                alternate_text: None,
            },
        ],
    });
    layout
}

pub(crate) fn paragraph(children: Vec<PositionedElement>) -> PositionedElement {
    PositionedElement::MarkedContent {
        structure: Some(structure_id(2)),
        children,
    }
}

/// The structure element `element` refers to, by its `/S` role.
pub(crate) fn role(read: &Pdf, element: &lopdf::Object) -> Vec<u8> {
    read.resolve(element)
        .as_dict()
        .unwrap()
        .get(b"S")
        .unwrap()
        .as_name()
        .unwrap()
        .to_vec()
}

/// The paragraph element's kids: `MCR` for marked content, else the role.
pub(crate) fn paragraph_kids(read: &Pdf) -> Vec<String> {
    let root = read.dict(read.catalog(), b"StructTreeRoot");
    let document = read.resolve(root.get(b"K").unwrap()).as_dict().unwrap();
    let paragraph = read
        .resolve(&document.get(b"K").unwrap().as_array().unwrap()[0])
        .as_dict()
        .unwrap();
    let kids = match read.resolve(paragraph.get(b"K").unwrap()) {
        lopdf::Object::Array(kids) => kids.clone(),
        other => vec![other.clone()],
    };
    kids.iter()
        .map(|kid| match read.resolve(kid) {
            lopdf::Object::Dictionary(dict) if dict.get(b"MCID").is_ok() => "MCR".to_owned(),
            lopdf::Object::Integer(_) => "MCR".to_owned(),
            _ => String::from_utf8(role(read, kid)).unwrap(),
        })
        .collect()
}

#[test]
fn a_tagged_widget_sits_in_a_form_element_of_its_paragraph() {
    let layout = tagged(with_fields(
        layout(vec![vec![paragraph(vec![
            label(),
            fragment(0, line(110.0, 100.0, 200.0), "Jane"),
        ])]]),
        vec![field("name", text_kind("Jane"))],
    ));
    let read = pdf(&layout);
    assert_eq!(paragraph_kids(&read), ["MCR", "Form"]);

    let widget_id = read.page_widgets(0)[0];
    let widget = read.doc.get_object(widget_id).unwrap().as_dict().unwrap();
    let key = widget.get(b"StructParent").unwrap().as_i64().unwrap();
    assert_eq!(key, 1, "keys after the one page's");

    let root = read.dict(read.catalog(), b"StructTreeRoot");
    assert_eq!(root.get(b"ParentTreeNextKey").unwrap().as_i64().unwrap(), 2);
    let nums = read
        .dict(root, b"ParentTree")
        .get(b"Nums")
        .unwrap()
        .as_array()
        .unwrap()
        .clone();
    let form_ref = nums[3].as_reference().unwrap();
    assert_eq!(nums[2].as_i64().unwrap(), key);
    let form = read.doc.get_object(form_ref).unwrap().as_dict().unwrap();
    assert_eq!(form.get(b"S").unwrap().as_name().unwrap(), b"Form");
    let objr = match read.resolve(form.get(b"K").unwrap()) {
        lopdf::Object::Array(kids) => {
            assert_eq!(kids.len(), 1);
            read.resolve(&kids[0]).as_dict().unwrap()
        }
        other => other.as_dict().unwrap(),
    };
    assert_eq!(objr.get(b"Type").unwrap().as_name().unwrap(), b"OBJR");
    assert_eq!(objr.get(b"Obj").unwrap().as_reference().unwrap(), widget_id);
    assert_eq!(
        objr.get(b"Pg").unwrap().as_reference().unwrap(),
        read.page_ids()[0]
    );

    let page = read
        .doc
        .get_object(read.page_ids()[0])
        .unwrap()
        .as_dict()
        .unwrap();
    assert_eq!(page.get(b"Tabs").unwrap().as_name().unwrap(), b"S");
}

#[test]
fn a_form_element_takes_its_place_among_the_paragraph_reading_order() {
    let layout = tagged(with_fields(
        layout(vec![vec![
            paragraph(vec![label()]),
            paragraph(vec![fragment(0, line(110.0, 100.0, 200.0), "Jane")]),
            paragraph(vec![run("after", Point { x: 72.0, y: 140.0 }, 11.0)]),
        ]]),
        vec![field("name", text_kind("Jane"))],
    ));
    assert_eq!(paragraph_kids(&pdf(&layout)), ["MCR", "Form", "MCR"]);
}

#[test]
fn a_fragment_outside_marked_content_is_tagged_under_the_document() {
    let layout = tagged(with_fields(
        layout(vec![vec![
            paragraph(vec![label()]),
            fragment(0, line(110.0, 100.0, 200.0), "Jane"),
        ]]),
        vec![field("name", text_kind("Jane"))],
    ));
    let read = pdf(&layout);
    let root = read.dict(read.catalog(), b"StructTreeRoot");
    let document = read.resolve(root.get(b"K").unwrap()).as_dict().unwrap();
    let roles: Vec<Vec<u8>> = document
        .get(b"K")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|kid| role(&read, kid))
        .collect();
    assert_eq!(roles, [b"P".to_vec(), b"Form".to_vec()]);
}

#[test]
fn a_tagged_field_without_tooltip_is_described_by_its_name() {
    let fields = vec![field("name", text_kind("Jane"))];
    let elements = vec![paragraph(vec![fragment(
        0,
        line(110.0, 100.0, 200.0),
        "Jane",
    )])];
    let tagged_read = pdf(&tagged(with_fields(
        layout(vec![elements.clone()]),
        fields.clone(),
    )));
    assert_eq!(text(tagged_read.field("name").get(b"TU").unwrap()), "name");
    let plain_read = pdf(&with_fields(layout(vec![elements]), fields));
    let plain = plain_read.field("name");
    assert!(plain.get(b"TU").is_err());
    assert!(plain.get(b"StructParent").is_err());
    let page = plain_read
        .doc
        .get_object(plain_read.page_ids()[0])
        .unwrap()
        .as_dict()
        .unwrap();
    assert!(page.get(b"Tabs").is_err(), "no tab order without structure");
}

#[test]
fn pdf_a_output_accepts_form_fields() {
    let layout = tagged(with_fields(
        layout(vec![vec![paragraph(vec![
            label(),
            fragment(0, line(110.0, 100.0, 200.0), "Jane"),
        ])]]),
        vec![field("name", text_kind("Jane"))],
    ));
    for profile in [PdfConformance::PdfA2b, PdfConformance::PdfA3b] {
        let bytes = render_to_pdf_with_options(&layout, PdfOptions::new(profile)).unwrap();
        let read = Pdf::read(&bytes);
        let field = read.field("name");
        assert_eq!(field.get(b"F").unwrap().as_i64().unwrap(), 4);
        assert_eq!(read.dict(field, b"AP").len(), 1);
        assert!(
            read.dict(read.catalog(), b"AcroForm")
                .get(b"NeedAppearances")
                .is_err()
        );
        assert_eq!(read.drawn_text(&read.appearance(field, None)), "Jane");
    }
}

/// A document element holding `paragraphs` paragraph elements, ids 2...
fn tagged_paragraphs(mut layout: LayoutResult, paragraphs: u32) -> LayoutResult {
    let mut nodes = vec![StructureNode {
        id: structure_id(1),
        role: StructureRole::Document,
        children: (2..2 + paragraphs).map(structure_id).collect(),
        alternate_text: None,
    }];
    nodes.extend((2..2 + paragraphs).map(|id| StructureNode {
        id: structure_id(id),
        role: StructureRole::Paragraph,
        children: Vec::new(),
        alternate_text: None,
    }));
    layout.structure = Some(DocumentStructure {
        root: structure_id(1),
        nodes,
    });
    layout
}

fn in_paragraph(id: u32, children: Vec<PositionedElement>) -> PositionedElement {
    PositionedElement::MarkedContent {
        structure: Some(structure_id(id)),
        children,
    }
}

#[test]
fn widget_keys_follow_every_page_key_and_name_their_own_form_element() {
    let mut name = field("name", text_kind("Jane"));
    name.tooltip = Some("Full name".to_owned());
    let layout = tagged_paragraphs(
        with_fields(
            layout(vec![
                vec![in_paragraph(
                    2,
                    vec![label(), fragment(0, line(110.0, 100.0, 200.0), "Jane")],
                )],
                vec![
                    in_paragraph(3, vec![fragment(1, line(72.0, 100.0, 200.0), "Lyon")]),
                    in_paragraph(4, vec![fragment(2, line(72.0, 130.0, 200.0), "France")]),
                ],
                vec![in_paragraph(
                    5,
                    vec![run("No field here", Point { x: 72.0, y: 112.0 }, 11.0)],
                )],
            ]),
            vec![
                name,
                field("city", text_kind("Lyon")),
                field("country", text_kind("France")),
            ],
        ),
        4,
    );
    let read = pdf(&layout);
    let root = read.dict(read.catalog(), b"StructTreeRoot");
    assert_eq!(
        root.get(b"ParentTreeNextKey").unwrap().as_i64().unwrap(),
        6,
        "3 pages, 3 widgets"
    );
    let nums = read
        .dict(root, b"ParentTree")
        .get(b"Nums")
        .unwrap()
        .as_array()
        .unwrap()
        .clone();
    let keys: Vec<i64> = nums
        .iter()
        .step_by(2)
        .map(|key| key.as_i64().unwrap())
        .collect();
    assert_eq!(
        keys,
        [0, 1, 2, 3, 4, 5],
        "sorted, pages first, no collision"
    );

    for (page, names) in [(0, vec!["name"]), (1, vec!["city", "country"])] {
        for widget_id in read.page_widgets(page) {
            let widget = read.doc.get_object(widget_id).unwrap().as_dict().unwrap();
            let key = widget.get(b"StructParent").unwrap().as_i64().unwrap();
            let position = keys.iter().position(|candidate| *candidate == key).unwrap();
            let form_id = nums[position * 2 + 1].as_reference().unwrap();
            let form = read.doc.get_object(form_id).unwrap().as_dict().unwrap();
            let objr = match read.resolve(form.get(b"K").unwrap()) {
                lopdf::Object::Array(kids) => read.resolve(&kids[0]).as_dict().unwrap(),
                other => other.as_dict().unwrap(),
            };
            assert_eq!(objr.get(b"Obj").unwrap().as_reference().unwrap(), widget_id);
            // The form's parent is the paragraph element that lists it.
            let parent_id = form.get(b"P").unwrap().as_reference().unwrap();
            let parent = read.doc.get_object(parent_id).unwrap().as_dict().unwrap();
            assert_eq!(parent.get(b"S").unwrap().as_name().unwrap(), b"P");
            let listed = match read.resolve(parent.get(b"K").unwrap()) {
                lopdf::Object::Array(kids) => kids.clone(),
                other => vec![other.clone()],
            };
            assert!(
                listed
                    .iter()
                    .any(|kid| kid.as_reference().ok() == Some(form_id))
            );
            assert!(names.contains(&text(widget.get(b"T").unwrap()).as_str()));
        }
    }
    assert_eq!(
        text(read.field("name").get(b"TU").unwrap()),
        "Full name",
        "the tooltip wins over the name"
    );
    let page = read
        .doc
        .get_object(read.page_ids()[2])
        .unwrap()
        .as_dict()
        .unwrap();
    assert!(
        page.get(b"Tabs").is_err(),
        "no tab order on a tagged page without widgets"
    );
}

#[test]
fn a_paragraph_holding_only_a_field_keeps_its_place_in_the_document() {
    let layout = tagged_paragraphs(
        with_fields(
            layout(vec![vec![
                in_paragraph(2, vec![fragment(0, line(72.0, 100.0, 200.0), "Jane")]),
                in_paragraph(3, vec![run("Next", Point { x: 72.0, y: 142.0 }, 11.0)]),
                fragment(1, line(72.0, 160.0, 200.0), "Loose"),
            ]]),
            vec![
                field("first", text_kind("Jane")),
                field("loose", text_kind("Loose")),
            ],
        ),
        2,
    );
    let read = pdf(&layout);
    let root = read.dict(read.catalog(), b"StructTreeRoot");
    let document = read.resolve(root.get(b"K").unwrap()).as_dict().unwrap();
    let kids = document.get(b"K").unwrap().as_array().unwrap();
    let roles: Vec<Vec<u8>> = kids.iter().map(|kid| role(&read, kid)).collect();
    assert_eq!(roles, [b"P".to_vec(), b"P".to_vec(), b"Form".to_vec()]);
    // The first paragraph is the one holding the first field.
    let first = read.resolve(&kids[0]).as_dict().unwrap();
    let held = match read.resolve(first.get(b"K").unwrap()) {
        lopdf::Object::Array(kids) => kids.clone(),
        other => vec![other.clone()],
    };
    assert_eq!(held.len(), 1);
    assert_eq!(role(&read, &held[0]), b"Form");
}

#[test]
fn a_field_in_a_list_item_sits_in_its_list_body() {
    let mut layout = with_fields(
        layout(vec![vec![PositionedElement::MarkedContent {
            structure: Some(structure_id(3)),
            children: vec![label(), fragment(0, line(110.0, 100.0, 200.0), "Jane")],
        }]]),
        vec![field("name", text_kind("Jane"))],
    );
    layout.structure = Some(DocumentStructure {
        root: structure_id(1),
        nodes: vec![
            StructureNode {
                id: structure_id(1),
                role: StructureRole::Document,
                children: vec![structure_id(2)],
                alternate_text: None,
            },
            StructureNode {
                id: structure_id(2),
                role: StructureRole::List,
                children: vec![structure_id(3)],
                alternate_text: None,
            },
            StructureNode {
                id: structure_id(3),
                role: StructureRole::ListItem,
                children: Vec::new(),
                alternate_text: None,
            },
        ],
    });
    let read = pdf(&layout);
    let widget = read
        .doc
        .get_object(read.page_widgets(0)[0])
        .unwrap()
        .as_dict()
        .unwrap();
    let key = widget.get(b"StructParent").unwrap().as_i64().unwrap();
    let root = read.dict(read.catalog(), b"StructTreeRoot");
    let nums = read
        .dict(root, b"ParentTree")
        .get(b"Nums")
        .unwrap()
        .as_array()
        .unwrap()
        .clone();
    let position = nums
        .iter()
        .step_by(2)
        .position(|candidate| candidate.as_i64().unwrap() == key)
        .unwrap();
    let form = read
        .doc
        .get_object(nums[position * 2 + 1].as_reference().unwrap())
        .unwrap()
        .as_dict()
        .unwrap();
    let parent = read
        .doc
        .get_object(form.get(b"P").unwrap().as_reference().unwrap())
        .unwrap()
        .as_dict()
        .unwrap();
    assert_eq!(parent.get(b"S").unwrap().as_name().unwrap(), b"LBody");
    let form_id = nums[position * 2 + 1].as_reference().unwrap();
    let listed = match read.resolve(parent.get(b"K").unwrap()) {
        lopdf::Object::Array(kids) => kids.clone(),
        other => vec![other.clone()],
    };
    assert!(
        listed
            .iter()
            .any(|kid| kid.as_reference().ok() == Some(form_id)),
        "the list body lists the form element"
    );
}

#[test]
fn fragments_with_no_marked_content_between_them_keep_their_page_order() {
    let layout = tagged_paragraphs(
        with_fields(
            layout(vec![vec![
                in_paragraph(2, vec![fragment(0, line(72.0, 100.0, 200.0), "First")]),
                fragment(1, line(72.0, 130.0, 200.0), "Loose"),
            ]]),
            vec![
                field("first", text_kind("First")),
                field("loose", text_kind("Loose")),
            ],
        ),
        1,
    );
    let read = pdf(&layout);
    let root = read.dict(read.catalog(), b"StructTreeRoot");
    let document = read.resolve(root.get(b"K").unwrap()).as_dict().unwrap();
    let kids = match read.resolve(document.get(b"K").unwrap()) {
        lopdf::Object::Array(kids) => kids.clone(),
        other => vec![other.clone()],
    };
    let roles: Vec<Vec<u8>> = kids.iter().map(|kid| role(&read, kid)).collect();
    assert_eq!(
        roles,
        [b"P".to_vec(), b"Form".to_vec()],
        "the paragraph's field comes first on the page"
    );
}

/// The operators of a content stream, decoded strictly: any byte that does
/// not parse fails the decoding.
fn operators(content: &[u8]) -> Vec<String> {
    lopdf::content::Content::decode_strict(content)
        .expect("the appearance parses to its last byte")
        .operations
        .into_iter()
        .map(|operation| operation.operator)
        .collect()
}

fn assert_operators_apart(content: &[u8]) {
    let operators = operators(content);
    assert!(
        operators.iter().all(|operator| matches!(
            operator.as_str(),
            "q" | "Q" | "cm" | "rg" | "BT" | "ET" | "Tf" | "Tm" | "TJ" | "Tj" | "gs"
        )),
        "every fragment's operators stay apart: {operators:?}"
    );
}

#[test]
fn the_fragments_of_a_field_draw_apart_in_every_appearance() {
    let check = |x: f64| {
        PositionedElement::FormField(FormFieldFragment {
            field: FormFieldId(1),
            rect: line(x, 130.0, 12.0),
            children: vec![run("X", Point { x, y: 142.0 }, 11.0)],
            alternate: vec![run("O", Point { x, y: 142.0 }, 11.0)],
            mark: None,
        })
    };
    let layout = with_fields(
        layout(vec![vec![
            fragment(0, line(72.0, 100.0, 60.0), "Jane"),
            fragment(0, line(132.0, 100.0, 60.0), "Doe"),
            check(72.0),
            check(90.0),
        ]]),
        vec![
            field("name", text_kind("Jane Doe")),
            field("agree", FormFieldKind::CheckBox { checked: true }),
        ],
    );
    let read = pdf(&layout);
    let name = read.appearance(read.field("name"), None);
    assert_operators_apart(&name);
    assert_eq!(read.drawn_text(&name), "JaneDoe");
    for state in [b"Yes".as_slice(), b"Off".as_slice()] {
        let appearance = read.appearance(read.field("agree"), Some(state));
        assert_operators_apart(&appearance);
        let texts = operators(&appearance)
            .iter()
            .filter(|operator| *operator == "BT")
            .count();
        assert_eq!(texts, 2, "both fragments of the state are drawn");
    }
}

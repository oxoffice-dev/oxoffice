//! Form field fragments become AcroForm fields with appearance streams.

use oxoffice_layout::{
    ChoiceOption, Color, FieldAlignment, FormField, FormFieldFragment, FormFieldId, FormFieldKind,
    GroupElement, LayoutResult, Point, PositionedElement, Rect, Transform,
};
use oxoffice_pdf::render_to_pdf_checked;

use crate::support::{PAGE_HEIGHT, Pdf, contains, layout, rect, run, text};

fn field(name: &str, kind: FormFieldKind) -> FormField {
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

fn text_kind(value: &str) -> FormFieldKind {
    FormFieldKind::Text {
        value: value.to_owned(),
        multiline: false,
        max_len: None,
    }
}

/// One line's fragment: the value drawn on a baseline 12 points under the
/// top of `rect`.
fn fragment(field: u32, rect: Rect, value: &str) -> PositionedElement {
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
    })
}

fn line(x: f64, y: f64, width: f64) -> Rect {
    Rect {
        x,
        y,
        width,
        height: 14.0,
    }
}

fn label() -> PositionedElement {
    run("Name:", Point { x: 72.0, y: 112.0 }, 11.0)
}

fn with_fields(mut layout: LayoutResult, fields: Vec<FormField>) -> LayoutResult {
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

fn pdf(layout: &LayoutResult) -> Pdf {
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
    assert_eq!(
        read.drawn_text(&read.appearance(read.field("notes"), None)),
        "first linesecond line"
    );
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

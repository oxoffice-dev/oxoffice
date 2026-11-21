//! Native charts on slides: adding each kind, reading its kind, categories,
//! series and values back, the embedded workbook, replacing the data, the
//! legend, axis titles, removing charts, several charts, copied slides, and
//! charts added to decks saved by PowerPoint and Google Slides.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Cursor;

use oxoffice_opc::OpcPackage;
use oxoffice_pptx::{
    ChartData, ChartKind, Emu, LegendPosition, LegendSettings, Presentation, RgbColor, ShapeType,
};
use quick_xml::Reader;
use quick_xml::events::Event;

use crate::support::{corpus_deck, new_deck, reopen, slide_texts};

const EVERY_KIND: [ChartKind; 7] = [
    ChartKind::Bar,
    ChartKind::Line,
    ChartKind::Pie,
    ChartKind::Doughnut,
    ChartKind::Area,
    ChartKind::Scatter,
    ChartKind::Radar,
];

/// A new deck with `count` slides on the default template's Blank layout.
fn blank_deck(count: usize) -> Presentation {
    let mut prs = new_deck();
    let blank = (0..prs.layout_count())
        .find(|&index| prs.layout_name(index) == Some("Blank"))
        .expect("the default template has a Blank layout");
    for _ in 0..count {
        prs.add_slide(blank).expect("blank slide is added");
    }
    prs
}

/// Adds a chart in a 6 x 4 inch frame and returns its shape id.
fn chart_on(prs: &mut Presentation, slide: usize, kind: ChartKind, data: &ChartData) -> u32 {
    prs.add_chart(
        slide,
        kind,
        Emu(914_400),
        Emu(1_371_600),
        Emu(5_486_400),
        Emu(3_657_600),
        data,
    )
    .unwrap_or_else(|e| panic!("{kind:?} chart is added: {e}"))
    .non_visual_id()
    .expect("a chart frame has a shape id")
}

fn data(categories: &[&str], series: &[(&str, &[f64])]) -> ChartData {
    ChartData {
        categories: categories.iter().map(|c| (*c).to_owned()).collect(),
        series: series
            .iter()
            .map(|(name, values)| ((*name).to_owned(), values.to_vec()))
            .collect(),
        ..ChartData::default()
    }
}

/// Two series over three regions.
fn regions() -> ChartData {
    data(
        &["North", "South", "West"],
        &[
            ("Revenue", &[12.5, 19.0, 14.25]),
            ("Cost", &[8.0, 11.5, 9.75]),
        ],
    )
}

/// One series over three regions, the shape a pie or doughnut takes.
fn shares() -> ChartData {
    data(
        &["North", "South", "West"],
        &[("Share", &[50.0, 30.0, 20.0])],
    )
}

/// The shape ids of a slide's top-level shapes, in z-order.
fn shape_ids(prs: &Presentation, slide: usize) -> Vec<u32> {
    prs.slide(slide)
        .expect("slide exists")
        .shapes()
        .filter_map(|shape| shape.non_visual_id())
        .collect()
}

/// The id of a slide's last top-level shape.
fn last_shape_id(prs: &Presentation, slide: usize) -> u32 {
    *shape_ids(prs, slide).last().expect("slide has a shape")
}

fn chart(prs: &Presentation, slide: usize, id: u32) -> (ChartKind, ChartData) {
    prs.chart_data(slide, id)
        .unwrap_or_else(|e| panic!("chart {id} on slide {slide} reads: {e}"))
}

fn legend(prs: &Presentation, slide: usize, id: u32) -> LegendSettings {
    prs.chart_legend(slide, id)
        .unwrap_or_else(|e| panic!("legend of chart {id} on slide {slide} reads: {e}"))
}

/// The package as saved.
fn saved_package(prs: &Presentation) -> OpcPackage {
    OpcPackage::from_reader(Cursor::new(prs.to_bytes().expect("deck saves")))
        .expect("saved package opens")
}

/// The saved chart and embedded workbook parts, by name.
fn chart_parts(prs: &Presentation) -> BTreeSet<String> {
    saved_package(prs)
        .parts
        .into_keys()
        .filter(|name| name.starts_with("/ppt/charts/") || name.starts_with("/ppt/embeddings/"))
        .collect()
}

/// The XML of the deck's only chart part.
fn only_chart_xml(prs: &Presentation) -> String {
    let package = saved_package(prs);
    let parts: Vec<_> = package
        .parts
        .iter()
        .filter(|(name, _)| name.starts_with("/ppt/charts/") && name.ends_with(".xml"))
        .collect();
    assert_eq!(parts.len(), 1, "one chart part");
    String::from_utf8(parts[0].1.to_vec()).expect("chart XML is UTF-8")
}

/// The text of every element named `local` in `xml`, in document order.
fn element_texts(xml: &[u8], local: &[u8]) -> Vec<String> {
    let mut reader = Reader::from_reader(xml);
    let mut texts = Vec::new();
    let mut current: Option<String> = None;
    let mut depth = 0usize;
    loop {
        match reader.read_event().expect("well-formed XML") {
            Event::Start(start) if start.local_name().as_ref() == local => {
                if current.is_none() {
                    current = Some(String::new());
                }
                depth += 1;
            }
            Event::End(end) if end.local_name().as_ref() == local && depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    texts.push(current.take().unwrap());
                }
            }
            Event::Empty(start) if start.local_name().as_ref() == local && depth == 0 => {
                texts.push(String::new());
            }
            Event::Text(text) => {
                if let Some(current) = &mut current {
                    current.push_str(&oxoffice_core::xml_text::decode_plain(&text));
                }
            }
            Event::GeneralRef(entity) => {
                if let Some(current) = &mut current {
                    current.push_str(&oxoffice_core::xml_text::resolve_entity(&entity));
                }
            }
            Event::Eof => return texts,
            _ => {}
        }
    }
}

/// The non-empty cells of the first sheet of one embedded workbook, by
/// address, with shared and inline strings resolved.
fn workbook_cells(xlsx: &[u8]) -> BTreeMap<String, String> {
    let workbook = OpcPackage::from_reader(Cursor::new(xlsx)).expect("embedded workbook opens");
    let shared = workbook
        .get_part("/xl/sharedStrings.xml")
        .map(|xml| element_texts(xml, b"si"))
        .unwrap_or_default();
    let sheet = workbook
        .get_part("/xl/worksheets/sheet1.xml")
        .expect("embedded workbook has a first sheet");
    let mut cells = BTreeMap::new();
    let mut reader = Reader::from_reader(sheet);
    loop {
        match reader.read_event().expect("well-formed sheet XML") {
            Event::Start(start) if start.local_name().as_ref() == b"c" => {
                let attribute = |name: &[u8]| {
                    start
                        .try_get_attribute(name)
                        .expect("cell attribute parses")
                        .map(|a| String::from_utf8(a.value.to_vec()).unwrap())
                };
                let address = attribute(b"r").expect("cell has an address");
                let kind = attribute(b"t");
                let end = start.to_end().into_owned();
                let inner = reader
                    .read_text(end.name())
                    .expect("cell content reads")
                    .into_inner();
                let inner = format!("<c>{}</c>", String::from_utf8_lossy(&inner));
                let value = match kind.as_deref() {
                    Some("s") => element_texts(inner.as_bytes(), b"v")
                        .first()
                        .map(|index| shared[index.parse::<usize>().unwrap()].clone()),
                    Some("inlineStr") => element_texts(inner.as_bytes(), b"is").first().cloned(),
                    _ => element_texts(inner.as_bytes(), b"v").first().cloned(),
                };
                if let Some(value) = value.filter(|value| !value.is_empty()) {
                    cells.insert(address, value);
                }
            }
            Event::Eof => return cells,
            _ => {}
        }
    }
}

/// The cells of every saved embedded workbook, by part name.
fn workbooks(prs: &Presentation) -> BTreeMap<String, BTreeMap<String, String>> {
    let package = saved_package(prs);
    package
        .parts
        .iter()
        .filter(|(name, _)| name.starts_with("/ppt/embeddings/") && name.ends_with(".xlsx"))
        .map(|(name, bytes)| (name.clone(), workbook_cells(bytes)))
        .collect()
}

fn only_workbook(prs: &Presentation) -> BTreeMap<String, String> {
    let books = workbooks(prs);
    assert_eq!(books.len(), 1, "one workbook: {:?}", books.keys());
    books.into_values().next().unwrap()
}

fn cells(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(address, value)| ((*address).to_owned(), (*value).to_owned()))
        .collect()
}

/// The `val` of each flag of the first plot-level `c:dLbls` of a chart part,
/// by flag name.
fn data_label_flags(xml: &str) -> BTreeMap<String, String> {
    let mut reader = Reader::from_reader(xml.as_bytes());
    let mut flags = BTreeMap::new();
    let mut depth = None;
    loop {
        match reader.read_event().expect("well-formed chart XML") {
            Event::Start(start) if start.local_name().as_ref() == b"dLbls" => depth = Some(0),
            Event::Start(_) => depth = depth.map(|d| d + 1),
            Event::End(end) if end.local_name().as_ref() == b"dLbls" => return flags,
            Event::End(_) => depth = depth.map(|d| d - 1),
            Event::Empty(flag) if depth == Some(0) => {
                let value = flag
                    .try_get_attribute("val")
                    .expect("flag attribute parses")
                    .map(|a| String::from_utf8(a.value.to_vec()).unwrap());
                let name = String::from_utf8(flag.local_name().as_ref().to_vec()).unwrap();
                flags.insert(name, value.unwrap_or_default());
            }
            Event::Eof => panic!("the chart has no data labels"),
            _ => {}
        }
    }
}

// --- Adding and reading back -------------------------------------------------

#[test]
fn a_bar_chart_reads_back_its_categories_series_and_values_after_a_save() {
    let mut prs = blank_deck(1);
    let id = chart_on(&mut prs, 0, ChartKind::Bar, &regions());

    let (kind, read) = chart(&reopen(&prs), 0, id);
    assert_eq!(kind, ChartKind::Bar);
    assert_eq!(read.categories, ["North", "South", "West"]);
    assert_eq!(
        read.series,
        [
            ("Revenue".to_owned(), vec![12.5, 19.0, 14.25]),
            ("Cost".to_owned(), vec![8.0, 11.5, 9.75]),
        ]
    );
    assert_eq!(read.number_format, None, "General reads back as no format");
}

#[test]
fn every_chart_kind_reads_back_with_its_kind_data_and_chart_type() {
    // Chart types are python-pptx XL_CHART_TYPE numbers: Bar is drawn as
    // clustered columns, Line and Scatter carry markers.
    let expected_type = |kind| match kind {
        ChartKind::Bar => 51,
        ChartKind::Line => 65,
        ChartKind::Pie => 5,
        ChartKind::Doughnut => -4120,
        ChartKind::Area => 1,
        ChartKind::Scatter => 74,
        ChartKind::Radar => 81,
    };
    // Numeric categories, so the same data suits the X axis of a scatter.
    let source = data(&["1", "2.5", "4"], &[("Sales", &[3.0, 7.0, 5.0])]);
    for kind in EVERY_KIND {
        let mut prs = blank_deck(1);
        let id = chart_on(&mut prs, 0, kind, &source);
        let prs = reopen(&prs);
        assert_eq!(chart(&prs, 0, id), (kind, source.clone()), "{kind:?}");
        assert_eq!(
            prs.chart_type(0, id).unwrap(),
            Some(expected_type(kind)),
            "{kind:?}"
        );
        assert!(prs.validate().is_empty(), "{kind:?}: {:?}", prs.validate());
    }
}

#[test]
fn a_chart_frame_sits_where_it_was_placed_and_can_be_moved_and_resized() {
    let mut prs = blank_deck(1);
    let id = prs
        .add_chart(
            0,
            ChartKind::Line,
            Emu(457_200),
            Emu(914_400),
            Emu(4_572_000),
            Emu(2_743_200),
            &regions(),
        )
        .unwrap()
        .non_visual_id()
        .unwrap();

    let saved = reopen(&prs);
    let shape = saved.slide(0).unwrap().shape(0).expect("the chart frame");
    assert_eq!(shape.shape_type(), Some(ShapeType::Chart));
    assert_eq!(shape.non_visual_id(), Some(id));
    assert_eq!(shape.non_visual_name(), Some(format!("Chart {id}")));
    assert_eq!(shape.position(), Some((Emu(457_200), Emu(914_400))));
    assert_eq!(shape.size(), Some((Emu(4_572_000), Emu(2_743_200))));

    let mut slide = prs.slide_mut(0).unwrap();
    let mut frame = slide.shape_mut(0).unwrap();
    frame.set_position(Emu(1_828_800), Emu(457_200)).unwrap();
    frame.set_size(Emu(6_400_800), Emu(3_200_400)).unwrap();
    let moved = reopen(&prs);
    let shape = moved.slide(0).unwrap().shape(0).unwrap();
    assert_eq!(shape.position(), Some((Emu(1_828_800), Emu(457_200))));
    assert_eq!(shape.size(), Some((Emu(6_400_800), Emu(3_200_400))));
    assert_eq!(chart(&moved, 0, id).1, regions());
}

#[test]
fn values_keep_their_full_precision_and_sign() {
    let values = [
        0.1 + 0.2,
        -1e-7,
        1_234_567.891,
        0.0,
        -42.5,
        1e15,
        f64::MIN_POSITIVE,
    ];
    let categories = ["a", "b", "c", "d", "e", "f", "g"];
    let source = data(&categories, &[("Measures", &values)]);
    let mut prs = blank_deck(1);
    let id = chart_on(&mut prs, 0, ChartKind::Line, &source);

    let (_, read) = chart(&reopen(&prs), 0, id);
    for (read, written) in read.series[0].1.iter().zip(values) {
        assert_eq!(read.to_bits(), written.to_bits(), "{written:e}");
    }
}

#[test]
fn unicode_right_to_left_and_cjk_labels_survive_a_save() {
    let categories = ["東京", "القاهرة", "Zürich", "Ⅲ 😀", "<b> & \"q\""];
    let source = data(
        &categories,
        &[
            ("売上高", &[1.0, 2.0, 3.0, 4.0, 5.0]),
            ("المبيعات €", &[5.0, 4.0, 3.0, 2.0, 1.0]),
        ],
    );
    let mut prs = blank_deck(1);
    let id = chart_on(&mut prs, 0, ChartKind::Bar, &source);

    assert_eq!(chart(&reopen(&prs), 0, id).1, source);
    // The embedded workbook, what "Edit Data" opens, carries them too.
    let book = only_workbook(&prs);
    for (row, category) in categories.iter().enumerate() {
        assert_eq!(book[&format!("A{}", row + 2)], *category);
    }
    assert_eq!(book["B1"], "売上高");
    assert_eq!(book["C1"], "المبيعات €");
}

#[test]
fn a_number_format_reads_back_from_the_chart() {
    for format in ["0.0%", "#,##0.00\" €\"", "0.00E+00"] {
        let mut source = shares();
        source.number_format = Some(format.to_owned());
        let mut prs = blank_deck(1);
        let id = chart_on(&mut prs, 0, ChartKind::Bar, &source);
        assert_eq!(chart(&reopen(&prs), 0, id).1, source, "{format}");
    }
}

#[test]
fn scatter_x_values_read_back_as_categories_and_can_be_replaced() {
    let source = data(&["-1.5", "0", "2.25"], &[("Y", &[4.0, 1.0, 9.0])]);
    let mut prs = blank_deck(1);
    let id = chart_on(&mut prs, 0, ChartKind::Scatter, &source);
    assert_eq!(chart(&reopen(&prs), 0, id), (ChartKind::Scatter, source));

    let replacement = data(
        &["10", "20", "30", "40"],
        &[("Y", &[1.0, 2.0, 3.0, 4.0]), ("Z", &[4.0, 3.0, 2.0, 1.0])],
    );
    prs.replace_chart_data(0, id, &replacement).unwrap();
    assert_eq!(
        chart(&reopen(&prs), 0, id),
        (ChartKind::Scatter, replacement)
    );
}

#[test]
fn the_embedded_workbook_holds_the_chart_data() {
    let mut prs = blank_deck(1);
    chart_on(&mut prs, 0, ChartKind::Bar, &regions());

    let mut book = only_workbook(&prs);
    book.remove("A1"); // the header above the categories
    assert_eq!(
        book,
        cells(&[
            ("A2", "North"),
            ("A3", "South"),
            ("A4", "West"),
            ("B1", "Revenue"),
            ("B2", "12.5"),
            ("B3", "19"),
            ("B4", "14.25"),
            ("C1", "Cost"),
            ("C2", "8"),
            ("C3", "11.5"),
            ("C4", "9.75"),
        ])
    );
}

#[test]
fn axis_titles_given_at_creation_are_written_on_the_axes() {
    // The facade has no reader for axis titles: read the chart part.
    let mut source = regions();
    source.category_axis_title = Some("Region".to_owned());
    source.value_axis_title = Some("Euros (k)".to_owned());
    let mut prs = blank_deck(1);
    chart_on(&mut prs, 0, ChartKind::Bar, &source);

    let xml = only_chart_xml(&reopen(&prs));
    let category_axis = &element_texts(xml.as_bytes(), b"catAx")[0];
    let value_axis = &element_texts(xml.as_bytes(), b"valAx")[0];
    assert!(category_axis.contains("Region"), "{xml}");
    assert!(value_axis.contains("Euros (k)"), "{xml}");
}

// Fails today: labels state only showPercent, so LibreOffice shows every field. Issue: #42
#[test]
#[should_panic(expected = "data labels leave a field to the reader")]
fn pie_and_doughnut_slices_are_labelled_with_their_percentage_only() {
    // The facade has no reader for data labels: read the chart part. Readers
    // disagree on an absent flag, so each field must be stated.
    for kind in [ChartKind::Pie, ChartKind::Doughnut] {
        let mut prs = blank_deck(1);
        chart_on(&mut prs, 0, kind, &shares());
        let flags = data_label_flags(&only_chart_xml(&prs));
        assert_eq!(flags.get("showPercent").map(String::as_str), Some("1"));
        for field in ["showLegendKey", "showVal", "showCatName", "showSerName"] {
            assert_eq!(
                flags.get(field).map(String::as_str),
                Some("0"),
                "{kind:?}: data labels leave a field to the reader: {field}"
            );
        }
    }
}

#[test]
fn a_palette_colours_the_series_in_order() {
    // The facade has no reader for series colours: read the chart part.
    let mut source = regions();
    source.palette = vec![
        RgbColor::new(0x1F, 0x77, 0xB4),
        RgbColor::new(0xFF, 0x7F, 0x0E),
    ];
    let mut prs = blank_deck(1);
    chart_on(&mut prs, 0, ChartKind::Bar, &source);

    let xml = only_chart_xml(&prs);
    let series = element_texts(xml.as_bytes(), b"ser");
    assert_eq!(series.len(), 2);
    let colours: Vec<_> = xml
        .match_indices("<c:ser")
        .map(|(start, _)| {
            let tail = &xml[start..];
            let colour = tail.find("srgbClr val=\"").expect("series has a colour") + 13;
            tail[colour..colour + 6].to_owned()
        })
        .collect();
    assert_eq!(colours, ["1F77B4", "FF7F0E"]);
}

// --- Refusals ----------------------------------------------------------------

#[test]
fn invalid_chart_data_is_refused_and_leaves_the_deck_unchanged() {
    let one = |category: &str, name: &str, value: f64| data(&[category], &[(name, &[value])]);
    let cases: Vec<(&str, ChartKind, ChartData)> = vec![
        ("no category", ChartKind::Bar, data(&[], &[("S", &[])])),
        ("no series", ChartKind::Bar, data(&["a"], &[])),
        (
            "fewer values than categories",
            ChartKind::Bar,
            data(&["a", "b"], &[("S", &[1.0])]),
        ),
        ("not a number", ChartKind::Line, one("a", "S", f64::NAN)),
        ("infinite", ChartKind::Line, one("a", "S", f64::INFINITY)),
        (
            "empty number format",
            ChartKind::Bar,
            ChartData {
                number_format: Some(String::new()),
                ..one("a", "S", 1.0)
            },
        ),
        ("pie with two series", ChartKind::Pie, regions()),
        ("doughnut with two series", ChartKind::Doughnut, regions()),
        (
            "scatter with a text X value",
            ChartKind::Scatter,
            one("x", "S", 1.0),
        ),
        (
            "control character in a category",
            ChartKind::Bar,
            one("a\u{1}", "S", 1.0),
        ),
        (
            "control character in a name",
            ChartKind::Bar,
            one("a", "S\u{b}", 1.0),
        ),
    ];
    let mut prs = blank_deck(1);
    let before = prs.to_bytes().unwrap();
    for (case, kind, data) in cases {
        let result = prs
            .add_chart(0, kind, Emu(0), Emu(0), Emu(914_400), Emu(914_400), &data)
            .map(|_| ());
        assert!(result.is_err(), "{case}: accepted");
        assert_eq!(prs.to_bytes().unwrap(), before, "{case}: deck changed");
    }
}

#[test]
fn a_chart_needs_a_positive_size_and_an_existing_slide() {
    let mut prs = blank_deck(1);
    let before = prs.to_bytes().unwrap();
    for (width, height) in [
        (0, 914_400),
        (914_400, 0),
        (-914_400, 914_400),
        (914_400, -1),
    ] {
        let result = prs
            .add_chart(
                0,
                ChartKind::Bar,
                Emu(0),
                Emu(0),
                Emu(width),
                Emu(height),
                &regions(),
            )
            .map(|_| ());
        assert!(result.is_err(), "{width} x {height}: accepted");
    }
    let result = prs
        .add_chart(
            1,
            ChartKind::Bar,
            Emu(0),
            Emu(0),
            Emu(914_400),
            Emu(914_400),
            &regions(),
        )
        .map(|_| ());
    assert!(result.is_err(), "slide 1 of 1: accepted");
    assert_eq!(prs.to_bytes().unwrap(), before);
}

#[test]
fn chart_calls_on_a_shape_that_is_not_a_chart_are_refused() {
    let mut prs = blank_deck(1);
    let chart_id = chart_on(&mut prs, 0, ChartKind::Bar, &regions());
    prs.slide_mut(0)
        .unwrap()
        .add_textbox(Emu(0), Emu(0), Emu(914_400), Emu(914_400))
        .unwrap()
        .set_text("Not a chart")
        .unwrap();
    let text_id = last_shape_id(&prs, 0);
    let before = prs.to_bytes().unwrap();

    for (slide, id) in [(0, text_id), (0, 999), (1, chart_id)] {
        let place = format!("shape {id} on slide {slide}");
        assert!(prs.chart_data(slide, id).is_err(), "{place}: data");
        assert!(prs.chart_type(slide, id).is_err(), "{place}: type");
        assert!(prs.chart_legend(slide, id).is_err(), "{place}: legend");
        assert!(
            prs.replace_chart_data(slide, id, &shares()).is_err(),
            "{place}: replace"
        );
        assert!(
            prs.set_chart_legend(slide, id, Some(false), None, None)
                .is_err(),
            "{place}: set legend"
        );
    }
    assert_eq!(prs.to_bytes().unwrap(), before);
}

// --- Legend ------------------------------------------------------------------

#[test]
fn a_chart_shows_its_legend_on_the_right_when_it_has_several_series_or_slices() {
    let shown = LegendSettings {
        visible: true,
        position: LegendPosition::Right,
        overlay: false,
    };
    for (kind, source, visible) in [
        (ChartKind::Bar, regions(), true),
        (ChartKind::Line, regions(), true),
        (ChartKind::Pie, shares(), true),
        (ChartKind::Doughnut, shares(), true),
        (ChartKind::Bar, shares(), false),
        (ChartKind::Line, shares(), false),
    ] {
        let mut prs = blank_deck(1);
        let id = chart_on(&mut prs, 0, kind, &source);
        let read = legend(&reopen(&prs), 0, id);
        assert_eq!(
            read.visible,
            visible,
            "{kind:?} with {} series",
            source.series.len()
        );
        if visible {
            assert_eq!(read, shown, "{kind:?}");
        }
    }
}

#[test]
fn the_legend_position_and_overlay_survive_a_save() {
    let mut prs = blank_deck(1);
    let id = chart_on(&mut prs, 0, ChartKind::Bar, &regions());
    for (position, overlay) in [
        (LegendPosition::Bottom, false),
        (LegendPosition::Top, true),
        (LegendPosition::Left, false),
        (LegendPosition::TopRight, true),
        (LegendPosition::Right, false),
    ] {
        prs.set_chart_legend(0, id, None, Some(position), Some(overlay))
            .unwrap();
        let expected = LegendSettings {
            visible: true,
            position,
            overlay,
        };
        assert_eq!(legend(&reopen(&prs), 0, id), expected);
    }

    // Asking for the current settings again changes nothing.
    let before = prs.to_bytes().unwrap();
    prs.set_chart_legend(0, id, Some(true), Some(LegendPosition::Right), Some(false))
        .unwrap();
    assert_eq!(prs.to_bytes().unwrap(), before);
}

#[test]
fn a_hidden_legend_stays_hidden_until_it_is_shown_again() {
    let mut prs = blank_deck(1);
    let id = chart_on(&mut prs, 0, ChartKind::Pie, &shares());
    prs.set_chart_legend(0, id, Some(false), None, None)
        .unwrap();
    assert!(!legend(&reopen(&prs), 0, id).visible);

    // Moving a hidden legend does not show it.
    prs.set_chart_legend(0, id, None, Some(LegendPosition::Bottom), Some(true))
        .unwrap();
    assert!(!legend(&reopen(&prs), 0, id).visible);

    prs.set_chart_legend(0, id, Some(true), Some(LegendPosition::Bottom), None)
        .unwrap();
    let read = legend(&reopen(&prs), 0, id);
    assert_eq!(
        read,
        LegendSettings {
            visible: true,
            position: LegendPosition::Bottom,
            overlay: false,
        }
    );
    assert_eq!(chart(&reopen(&prs), 0, id).1, shares());
}

// --- Replacing data ----------------------------------------------------------

#[test]
fn replacing_chart_data_can_add_and_drop_series_and_categories() {
    let mut prs = blank_deck(1);
    let id = chart_on(&mut prs, 0, ChartKind::Line, &regions());
    let mut prs = reopen(&prs); // a later session edits the chart

    let larger = data(
        &["Q1", "Q2", "Q3", "Q4", "Q5"],
        &[
            ("2024", &[1.0, 2.0, 3.0, 4.0, 5.0]),
            ("2025", &[2.0, 3.0, 4.0, 5.0, 6.0]),
            ("2026", &[3.0, 4.0, 5.0, 6.0, 7.0]),
        ],
    );
    prs.replace_chart_data(0, id, &larger).unwrap();
    assert_eq!(chart(&reopen(&prs), 0, id), (ChartKind::Line, larger));

    let smaller = data(&["Total"], &[("All years", &[42.0])]);
    prs.replace_chart_data(0, id, &smaller).unwrap();
    let saved = reopen(&prs);
    assert_eq!(chart(&saved, 0, id), (ChartKind::Line, smaller));
    assert!(saved.validate().is_empty(), "{:?}", saved.validate());
}

#[test]
fn replaced_data_reaches_the_embedded_workbook() {
    let mut prs = blank_deck(1);
    let id = chart_on(&mut prs, 0, ChartKind::Bar, &regions());

    let replacement = data(
        &["Lyon", "Nantes"],
        &[
            ("Units", &[7.0, 3.0]),
            ("Returns", &[1.0, 0.5]),
            ("Net", &[6.0, 2.5]),
        ],
    );
    prs.replace_chart_data(0, id, &replacement).unwrap();
    let mut book = only_workbook(&prs);
    book.remove("A1");
    assert_eq!(
        book,
        cells(&[
            ("A2", "Lyon"),
            ("A3", "Nantes"),
            ("B1", "Units"),
            ("B2", "7"),
            ("B3", "3"),
            ("C1", "Returns"),
            ("C2", "1"),
            ("C3", "0.5"),
            ("D1", "Net"),
            ("D2", "6"),
            ("D3", "2.5"),
        ]),
        "the old West row and the old names are gone"
    );
}

#[test]
fn replacing_chart_data_keeps_the_legend_and_axis_titles() {
    let mut source = regions();
    source.category_axis_title = Some("Region".to_owned());
    source.value_axis_title = Some("Euros".to_owned());
    let mut prs = blank_deck(1);
    let id = chart_on(&mut prs, 0, ChartKind::Bar, &source);
    prs.set_chart_legend(0, id, None, Some(LegendPosition::Bottom), None)
        .unwrap();

    prs.replace_chart_data(0, id, &shares()).unwrap();
    let saved = reopen(&prs);
    assert_eq!(chart(&saved, 0, id).1, shares());
    assert_eq!(legend(&saved, 0, id).position, LegendPosition::Bottom);
    let xml = only_chart_xml(&saved);
    assert!(
        element_texts(xml.as_bytes(), b"catAx")[0].contains("Region"),
        "{xml}"
    );
    assert!(
        element_texts(xml.as_bytes(), b"valAx")[0].contains("Euros"),
        "{xml}"
    );
}

#[test]
fn invalid_replacement_data_is_refused_and_keeps_the_old_data() {
    let mut prs = blank_deck(1);
    let bar = chart_on(&mut prs, 0, ChartKind::Bar, &regions());
    let pie = chart_on(&mut prs, 0, ChartKind::Pie, &shares());
    let before = prs.to_bytes().unwrap();

    let cases = [
        ("not a number", bar, data(&["a"], &[("S", &[f64::NAN])])),
        ("fewer values", bar, data(&["a", "b"], &[("S", &[1.0])])),
        ("no category", bar, data(&[], &[("S", &[])])),
        (
            "control character",
            bar,
            data(&["a\u{1}"], &[("S", &[1.0])]),
        ),
        (
            "axis title",
            bar,
            ChartData {
                value_axis_title: Some("Euros".to_owned()),
                ..shares()
            },
        ),
        (
            "palette",
            bar,
            ChartData {
                palette: vec![RgbColor::new(0, 0, 0)],
                ..shares()
            },
        ),
        ("pie with two series", pie, regions()),
    ];
    for (case, id, replacement) in cases {
        assert!(
            prs.replace_chart_data(0, id, &replacement).is_err(),
            "{case}: accepted"
        );
        assert_eq!(prs.to_bytes().unwrap(), before, "{case}: deck changed");
    }
    assert_eq!(chart(&prs, 0, bar).1, regions());
    assert_eq!(chart(&prs, 0, pie).1, shares());
}

#[test]
fn replacing_chart_data_with_the_same_data_changes_nothing() {
    let mut prs = blank_deck(1);
    let id = chart_on(&mut prs, 0, ChartKind::Area, &regions());
    let before = prs.to_bytes().unwrap();
    let (_, current) = chart(&prs, 0, id);
    prs.replace_chart_data(0, id, &current).unwrap();
    assert_eq!(prs.to_bytes().unwrap(), before);
}

// --- Several charts ----------------------------------------------------------

#[test]
fn several_charts_on_one_slide_keep_their_own_data_and_parts() {
    let trend = data(&["Jan", "Feb"], &[("Visits", &[120.0, 180.0])]);
    let mut prs = blank_deck(1);
    let bar = chart_on(&mut prs, 0, ChartKind::Bar, &regions());
    let pie = chart_on(&mut prs, 0, ChartKind::Pie, &shares());
    let line = chart_on(&mut prs, 0, ChartKind::Line, &trend);

    let saved = reopen(&prs);
    assert_eq!(shape_ids(&saved, 0), [bar, pie, line]);
    assert_eq!(chart(&saved, 0, bar), (ChartKind::Bar, regions()));
    assert_eq!(chart(&saved, 0, pie), (ChartKind::Pie, shares()));
    assert_eq!(chart(&saved, 0, line), (ChartKind::Line, trend));
    let parts = chart_parts(&saved);
    let charts = parts
        .iter()
        .filter(|p| p.starts_with("/ppt/charts/"))
        .count();
    let books = parts
        .iter()
        .filter(|p| p.starts_with("/ppt/embeddings/"))
        .count();
    assert_eq!((charts, books), (3, 3), "{parts:?}");
}

#[test]
fn editing_one_chart_leaves_the_other_charts_alone() {
    let mut prs = blank_deck(2);
    let first = chart_on(&mut prs, 0, ChartKind::Bar, &regions());
    let second = chart_on(&mut prs, 0, ChartKind::Bar, &regions());
    let elsewhere = chart_on(&mut prs, 1, ChartKind::Bar, &regions());

    prs.replace_chart_data(0, second, &shares()).unwrap();
    prs.set_chart_legend(0, second, Some(false), None, None)
        .unwrap();
    let saved = reopen(&prs);
    assert_eq!(chart(&saved, 0, second).1, shares());
    assert!(!legend(&saved, 0, second).visible);
    for (slide, id) in [(0, first), (1, elsewhere)] {
        assert_eq!(chart(&saved, slide, id).1, regions(), "chart {id}");
        assert!(legend(&saved, slide, id).visible, "chart {id}");
    }
}

// --- Removing ----------------------------------------------------------------

#[test]
fn removing_a_chart_drops_its_chart_part_and_workbook() {
    let mut prs = blank_deck(1);
    chart_on(&mut prs, 0, ChartKind::Bar, &regions());
    let kept = chart_on(&mut prs, 0, ChartKind::Pie, &shares());
    let before = chart_parts(&prs);

    prs.remove_shape(0, 0).unwrap();
    let saved = reopen(&prs);
    assert_eq!(shape_ids(&saved, 0), [kept]);
    assert_eq!(chart(&saved, 0, kept), (ChartKind::Pie, shares()));
    let after = chart_parts(&saved);
    assert_eq!(after.len(), 2, "one chart and its workbook left: {after:?}");
    assert!(after.is_subset(&before));
    assert!(saved.validate().is_empty(), "{:?}", saved.validate());
}

// Fails today: remove_slide prunes only media, the chart and workbook stay. Issue: #38
#[test]
#[should_panic(expected = "the removed slide's chart parts are still in the file")]
fn removing_the_slide_that_holds_a_chart_drops_its_chart_part_and_workbook() {
    let mut prs = blank_deck(2);
    chart_on(&mut prs, 1, ChartKind::Bar, &regions());

    prs.remove_slide(1).unwrap();
    let parts = chart_parts(&reopen(&prs));
    assert!(
        parts.is_empty(),
        "the removed slide's chart parts are still in the file: {parts:?}"
    );
}

#[test]
fn a_carriage_return_in_chart_text_is_saved_so_that_other_readers_keep_it() {
    let mut prs = blank_deck(1);
    let labels = data(
        &["North\rcoast", "South"],
        &[("Revenue\r2026", &[1.0, 2.0])],
    );
    let id = chart_on(&mut prs, 0, ChartKind::Bar, &labels);

    let saved = reopen(&prs);
    assert_eq!(chart(&saved, 0, id), (ChartKind::Bar, labels));
    let xml = only_chart_xml(&saved);
    assert!(xml.contains("North&#13;coast"), "{xml}");
    assert!(!xml.contains('\r'), "{xml}");
}

#[test]
fn a_raw_carriage_return_in_saved_chart_text_reads_as_a_line_feed() {
    let mut prs = blank_deck(1);
    let id = chart_on(&mut prs, 0, ChartKind::Bar, &regions());
    let mut package = saved_package(&prs);
    let part = package
        .parts
        .keys()
        .find(|name| name.starts_with("/ppt/charts/") && name.ends_with(".xml"))
        .cloned()
        .unwrap();
    let xml = String::from_utf8(package.get_part(&part).unwrap().to_vec()).unwrap();
    let patched = xml.replace("<c:v>North</c:v>", "<c:v>North\r\ncoast</c:v>");
    assert_ne!(patched, xml, "the cache names the category");
    package.set_part(&part, patched.into_bytes());
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).unwrap();
    let prs = Presentation::from_bytes(&bytes.into_inner()).unwrap();

    let (_, data) = chart(&prs, 0, id);
    assert_eq!(data.categories[0], "North\ncoast");
    let saved = reopen(&prs);
    assert_eq!(chart(&saved, 0, id).1.categories[0], "North\ncoast");
}

// --- Copies, groups ----------------------------------------------------------

#[test]
fn a_duplicated_slide_keeps_its_chart_and_edits_to_the_copy_leave_the_original_alone() {
    let mut prs = blank_deck(1);
    let original = chart_on(&mut prs, 0, ChartKind::Bar, &regions());
    prs.duplicate_slide(0).unwrap();
    let copy = last_shape_id(&prs, 1);
    assert_eq!(chart(&reopen(&prs), 1, copy), (ChartKind::Bar, regions()));

    prs.set_chart_legend(1, copy, Some(false), None, None)
        .unwrap();
    prs.replace_chart_data(1, copy, &shares()).unwrap();
    let saved = reopen(&prs);
    assert_eq!(chart(&saved, 1, copy).1, shares());
    assert!(!legend(&saved, 1, copy).visible);
    assert_eq!(chart(&saved, 0, original).1, regions());
    assert!(legend(&saved, 0, original).visible);
    // Each chart has a workbook of its own with its own data.
    let names: BTreeSet<_> = workbooks(&saved)
        .into_values()
        .map(|book| (book["B1"].clone(), book.get("C1").cloned()))
        .collect();
    assert_eq!(
        names,
        BTreeSet::from([
            ("Revenue".to_owned(), Some("Cost".to_owned())),
            ("Share".to_owned(), None),
        ])
    );
}

#[test]
fn an_imported_slide_brings_its_chart_and_each_import_is_its_own_copy() {
    let mut source = blank_deck(1);
    chart_on(&mut source, 0, ChartKind::Doughnut, &shares());
    let mut prs = corpus_deck("powerpoint/deck.pptx");
    let first = prs.len();

    prs.import_slide(&source, 0, None, None).unwrap();
    prs.import_slide(&source, 0, None, None).unwrap();
    let (a, b) = (last_shape_id(&prs, first), last_shape_id(&prs, first + 1));
    prs.replace_chart_data(first + 1, b, &data(&["Only"], &[("All", &[1.0])]))
        .unwrap();

    let saved = reopen(&prs);
    assert_eq!(chart(&saved, first, a), (ChartKind::Doughnut, shares()));
    assert_eq!(
        chart(&saved, first + 1, b).1,
        data(&["Only"], &[("All", &[1.0])])
    );
    assert_eq!(chart_parts(&saved).len(), 4, "two charts, two workbooks");
    assert!(saved.validate().is_empty(), "{:?}", saved.validate());
}

#[test]
fn a_chart_in_a_group_keeps_its_data_and_stays_editable() {
    let mut prs = blank_deck(1);
    let id = chart_on(&mut prs, 0, ChartKind::Bar, &regions());
    prs.slide_mut(0)
        .unwrap()
        .add_textbox(Emu(0), Emu(0), Emu(914_400), Emu(457_200))
        .unwrap()
        .set_text("Source: finance")
        .unwrap();
    prs.shapes_mut(0, &[]).unwrap().group(&[0, 1]).unwrap();
    let group = prs.slide(0).unwrap().shape(0).unwrap();
    assert_eq!(group.shape_type(), Some(ShapeType::Group));

    assert_eq!(chart(&prs, 0, id).1, regions());
    prs.replace_chart_data(0, id, &shares()).unwrap();
    prs.set_chart_legend(0, id, None, Some(LegendPosition::Top), None)
        .unwrap();
    let saved = reopen(&prs);
    assert_eq!(chart(&saved, 0, id).1, shares());
    assert_eq!(legend(&saved, 0, id).position, LegendPosition::Top);

    // Removing the group removes the chart with it.
    prs.remove_shape(0, 0).unwrap();
    assert!(chart_parts(&prs).is_empty(), "{:?}", chart_parts(&prs));
}

// --- Round trips, export, render ---------------------------------------------

#[test]
fn an_unedited_chart_is_saved_byte_for_byte() {
    let mut prs = blank_deck(1);
    chart_on(&mut prs, 0, ChartKind::Radar, &regions());
    let first = saved_package(&prs);
    let again = saved_package(&reopen(&reopen(&prs)));
    let names = chart_parts(&prs);
    assert_eq!(names.len(), 2);
    for name in names {
        assert_eq!(first.get_part(&name), again.get_part(&name), "{name}");
    }
}

#[test]
fn a_chart_exports_to_markdown_as_a_data_table() {
    let mut prs = blank_deck(1);
    chart_on(&mut prs, 0, ChartKind::Bar, &regions());
    // Decimal points come escaped (`12\.5`), which renders as `12.5`.
    let markdown = reopen(&prs).to_markdown().unwrap().text.replace("\\.", ".");
    for row in [
        "|  | Revenue | Cost |",
        "| North | 12.5 | 8 |",
        "| South | 19 | 11.5 |",
        "| West | 14.25 | 9.75 |",
    ] {
        assert!(markdown.contains(row), "{row} in:\n{markdown}");
    }
}

#[test]
fn every_chart_kind_renders_as_a_chart_not_a_placeholder() {
    let source = data(&["1", "2", "3"], &[("Sales", &[3.0, 7.0, 5.0])]);
    for kind in EVERY_KIND {
        let mut prs = blank_deck(1);
        chart_on(&mut prs, 0, kind, &source);
        let (png, diagnostics) = prs
            .render_slide_to_png_with_diagnostics(0, 48.0, &[])
            .unwrap();
        let png = png.expect("the slide renders");
        assert!(png.starts_with(b"\x89PNG"), "{kind:?}");
        let chart_notes: Vec<_> = diagnostics
            .iter()
            .map(|d| format!("{d:?}"))
            .filter(|d| d.contains("chart"))
            .collect();
        assert!(chart_notes.is_empty(), "{kind:?}: {chart_notes:?}");
    }
}

// --- Decks saved by other producers ------------------------------------------

#[test]
fn a_chart_added_to_the_powerpoint_deck_reads_back_and_leaves_the_rest_as_it_was() {
    let mut prs = corpus_deck("powerpoint/deck.pptx");
    let texts = slide_texts(&prs);
    let parts_before: BTreeSet<_> = saved_package(&prs).parts.into_keys().collect();
    let slide = 3; // "Revenue by quarter": a title, a picture and a text box
    let types_before: Vec<_> = prs
        .slide(slide)
        .unwrap()
        .shapes()
        .map(|s| s.shape_type())
        .collect();

    let quarters = data(
        &["Q1", "Q2", "Q3", "Q4"],
        &[("2025", &[1.2, 1.5, 1.7, 2.1])],
    );
    let id = chart_on(&mut prs, slide, ChartKind::Line, &quarters);

    let saved = reopen(&prs);
    assert_eq!(chart(&saved, slide, id), (ChartKind::Line, quarters));
    assert_eq!(slide_texts(&saved), texts);
    let types_after: Vec<_> = saved
        .slide(slide)
        .unwrap()
        .shapes()
        .map(|s| s.shape_type())
        .collect();
    assert_eq!(types_after[..types_before.len()], types_before[..]);
    assert_eq!(types_after.last(), Some(&Some(ShapeType::Chart)));
    let parts_after: BTreeSet<_> = saved_package(&saved).parts.into_keys().collect();
    assert!(parts_before.is_subset(&parts_after), "a part was lost");
    assert!(saved.validate().is_empty(), "{:?}", saved.validate());
}

#[test]
fn a_chart_added_to_the_google_slides_deck_gets_a_shape_id_of_its_own() {
    let mut prs = corpus_deck("google/deck.pptx");
    let slide = 3;
    let existing = shape_ids(&prs, slide);
    let id = chart_on(&mut prs, slide, ChartKind::Pie, &shares());
    assert!(!existing.contains(&id), "{id} in {existing:?}");

    let saved = reopen(&prs);
    let ids = shape_ids(&saved, slide);
    let unique: BTreeSet<_> = ids.iter().collect();
    assert_eq!(unique.len(), ids.len(), "{ids:?}");
    assert_eq!(chart(&saved, slide, id), (ChartKind::Pie, shares()));
    assert!(saved.validate().is_empty(), "{:?}", saved.validate());
}

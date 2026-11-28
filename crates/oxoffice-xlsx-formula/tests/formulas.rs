//! Formulas: every formula of the corpus, and one table per behaviour
//! (`tests/*.tsv`, a case per line, lines starting with `# ` are comments:
//! a case may start with `#REF!`).

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use oxoffice_xlsx_formula::{
    check_stored, function, move_references, result_extent, to_displayed, to_stored, tokenize,
};
use quick_xml::events::Event;

/// The cases of a table: its lines but comments, split at tabs.
fn cases(table: &str) -> impl Iterator<Item = Vec<&str>> {
    table
        .lines()
        .filter(|line| !line.starts_with("# "))
        .map(|line| line.split('\t').collect())
}

/// The elements that hold a formula in the parts of a workbook.
const FORMULA_ELEMENTS: [&str; 7] = [
    "f",
    "definedName",
    "formula",
    "formula1",
    "formula2",
    "calculatedColumnFormula",
    "totalsRowFormula",
];

/// Every formula of a workbook, with the part it is in.
fn formulas_of(path: &Path) -> Vec<(String, String)> {
    let mut archive = zip::ZipArchive::new(File::open(path).unwrap()).unwrap();
    let mut found = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let part = entry.name().unwrap().into_owned();
        if !part.ends_with(".xml") {
            continue;
        }
        let mut xml = String::new();
        entry.read_to_string(&mut xml).unwrap();
        let mut reader = quick_xml::Reader::from_str(&xml);
        let mut text: Option<String> = None;
        loop {
            match reader.read_event().unwrap() {
                Event::Start(start) => {
                    let name = start.local_name();
                    let local = std::str::from_utf8(name.as_ref()).unwrap();
                    text = FORMULA_ELEMENTS.contains(&local).then(String::new);
                }
                Event::Text(value) => {
                    if let Some(text) = &mut text {
                        text.push_str(&value.decode().unwrap());
                    }
                }
                Event::GeneralRef(reference) => {
                    if let Some(text) = &mut text {
                        text.push(match reference.resolve_char_ref().unwrap() {
                            Some(c) => c,
                            None => match &*reference.decode().unwrap() {
                                "amp" => '&',
                                "lt" => '<',
                                "gt" => '>',
                                "quot" => '"',
                                "apos" => '\'',
                                other => panic!("entity {other} in {part}"),
                            },
                        });
                    }
                }
                Event::End(_) => {
                    if let Some(text) = text.take().filter(|text| !text.is_empty()) {
                        found.push((part.clone(), text));
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
    }
    found
}

fn corpus_workbooks() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/files");
    let mut workbooks = Vec::new();
    for producer in fs::read_dir(root).unwrap() {
        for file in fs::read_dir(producer.unwrap().path()).unwrap() {
            let path = file.unwrap().path();
            if path
                .extension()
                .is_some_and(|extension| extension == "xlsx")
            {
                workbooks.push(path);
            }
        }
    }
    workbooks.sort();
    workbooks
}

#[test]
fn every_corpus_formula_comes_back_through_its_displayed_form() {
    let mut count = 0;
    let mut failures = Vec::new();
    for workbook in corpus_workbooks() {
        let name = workbook.file_name().unwrap().to_string_lossy().into_owned();
        for (part, formula) in formulas_of(&workbook) {
            count += 1;
            let tokens = tokenize(&formula).unwrap_or_default();
            let joined: String = tokens
                .iter()
                .map(|token| &formula[token.span.clone()])
                .collect();
            let back = to_stored(&to_displayed(&formula));
            if joined != formula || back.as_deref() != Ok(formula.as_str()) {
                failures.push(format!(
                    "{name} {part} {formula:?}: tokens {joined:?}, back {back:?}"
                ));
            } else if let Err(error) = check_stored(&formula) {
                failures.push(format!("{name} {part} {formula:?}: {error}"));
            }
        }
    }
    assert!(count > 400, "the walk found {count} formulas");
    assert!(
        failures.is_empty(),
        "{} formulas:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn the_lexer_tells_references_names_and_tables_apart() {
    for case in cases(include_str!("tokens.tsv")) {
        let [formula, kinds] = case[..] else {
            panic!("{case:?}")
        };
        let tokens = tokenize(formula).unwrap();
        let found: Vec<String> = tokens
            .iter()
            .map(|token| format!("{:?}", token.kind))
            .collect();
        assert_eq!(found.join(" "), kinds, "{formula}");
    }
}

#[test]
fn stored_and_displayed_forms_map_both_ways() {
    for case in cases(include_str!("forms.tsv")) {
        let [direction, stored, displayed] = case[..] else {
            panic!("{case:?}")
        };
        assert_eq!(to_stored(displayed).as_deref(), Ok(stored), "{displayed}");
        if direction == "both" {
            assert_eq!(to_displayed(stored), displayed, "{stored}");
            assert_eq!(check_stored(stored), Ok(()), "{stored}");
        }
    }
    // A formula this crate cannot read is shown as stored; a formula not in
    // its stored form is refused with that form.
    assert_eq!(to_displayed("SUM(A1"), "SUM(A1");
    assert_eq!(
        check_stored("XLOOKUP(A1,B:B,C:C)").unwrap_err().to_string(),
        "Excel stores this formula as `_xlfn.XLOOKUP(A1,B:B,C:C)` (at character 1)"
    );
}

#[test]
fn shared_formula_members_move_their_relative_references() {
    for case in cases(include_str!("moves.tsv")) {
        let [master, rows, columns, member] = case[..] else {
            panic!("{case:?}")
        };
        let (rows, columns) = (rows.parse().unwrap(), columns.parse().unwrap());
        assert_eq!(
            move_references(master, rows, columns).as_deref(),
            Ok(member),
            "{master}"
        );
    }
}

#[test]
fn a_result_is_as_large_as_its_formula_and_inputs_say() {
    let names = |sheet: Option<&str>, name: &str| {
        let refers = match (sheet, name) {
            (None, "Sales") => "Data!$B$2:$B$25",
            (None, "Rate") => "0.07",
            (Some("Data" | "My data"), "Local") => "$A$1:$C$3",
            (None, "Loop") => "Loop+1",
            (None, "Calc") => "OFFSET(A1,0,0,B1)",
            _ => return None,
        };
        Some(refers.to_owned())
    };
    for case in cases(include_str!("extents.tsv")) {
        let [formula, expected] = case[..] else {
            panic!("{case:?}")
        };
        let found = result_extent(formula, names).map_or("-".to_owned(), |extent| {
            let bound = if extent.exact { "" } else { "<=" };
            format!("{bound}{}x{}", extent.shape.rows, extent.shape.columns)
        });
        assert_eq!(found, expected, "{formula}");
    }
}

#[test]
fn the_syntax_check_refuses_what_excel_would_repair() {
    let nested = |levels| format!("{}1{}", "ABS(".repeat(levels), ")".repeat(levels));
    let arguments = |count| format!("SUM({})", vec!["1"; count].join(","));
    let long = format!("\"{}\"", "a".repeat(8_191));
    let generated = [
        (
            arguments(256),
            "a function takes at most 255 arguments, not 256 (at character 1)",
        ),
        (
            nested(65),
            "functions nest at most 64 levels deep (at character 257)",
        ),
        (
            long,
            "a formula holds at most 8,192 characters (at character 1)",
        ),
        (
            format!("SUM(1){}", "()".repeat(300)),
            "the formula nests too deeply (at character 517)",
        ),
    ];
    let table = cases(include_str!("refusals.tsv")).map(|case| (case[0].to_owned(), case[1]));
    for (formula, message) in table.chain(generated) {
        let error = to_stored(&formula).expect_err(&formula);
        assert_eq!(error.to_string(), message, "{formula}");
    }
    // Read from a file, any depth comes back as stored, on a small stack.
    let chained = format!("SUM(1){}", "()".repeat(100_000));
    let read = std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || to_displayed(&chained) == chained)
        .unwrap();
    assert!(read.join().unwrap());
    for formula in [
        nested(64),
        arguments(255),
        "IF(A1,,)".to_owned(),
        "SHEET()".to_owned(),
        "FILTER(A1:A9,B1:B9)".to_owned(),
        "SUM((A1,B1),C1 D1)".to_owned(),
        "MyUdf(1,2,3,4,5)".to_owned(),
        "-@A1:A10%".to_owned(),
    ] {
        assert!(
            to_stored(&formula).is_ok(),
            "{formula}: {:?}",
            to_stored(&formula)
        );
    }
    let xlookup = function("xlookup").unwrap();
    assert_eq!(
        (
            xlookup.prefix.as_str(),
            xlookup.min_arguments,
            xlookup.max_arguments
        ),
        ("_xlfn.", 3, 6)
    );
    assert_eq!(function("_xlfn.SUM"), None);
}

/// A small deterministic generator (xorshift) for the random tests.
struct Random(u64);

impl Random {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

/// A random displayed formula: references (3D, deleted, not in ASCII),
/// names, constants, operators, calls, LET and LAMBDA, `@` and `#`.
fn random_formula(random: &mut Random, depth: usize, parameters: &mut Vec<&'static str>) -> String {
    const LEAVES: [&str; 22] = [
        "A1",
        "$B$2",
        "C3:D4",
        "Jan:Dec!B2",
        "'Лист 1'!B2",
        "Лист1!Итого",
        "合計",
        "#REF!A1",
        "A:A",
        "2:3",
        "Rate",
        "Tbl[Col]",
        "Tbl[@Col]",
        "1",
        "2.5",
        "\"тест\"",
        "TRUE",
        "#N/A",
        "{1,2;3,4}",
        "A1#",
        "Data#",
        "SUM",
    ];
    if depth == 0 || random.below(4) == 0 {
        if !parameters.is_empty() && random.below(2) == 0 {
            return random.pick(parameters).to_owned();
        }
        return random.pick(&LEAVES).to_owned();
    }
    let depth = depth - 1;
    match random.below(10) {
        0 | 1 => {
            let left = random_formula(random, depth, parameters);
            let op = random.pick(&["+", "-", "*", "&", "<>", " ", ":", "^"]);
            format!("{left}{op}{}", random_formula(random, depth, parameters))
        }
        2 => format!("-{}", random_formula(random, depth, parameters)),
        3 => format!("({})", random_formula(random, depth, parameters)),
        4 => format!("@{}", random_formula(random, depth, parameters)),
        5 => format!("{}%", random_formula(random, depth, parameters)),
        6 => {
            let name = random.pick(&["SUM", "XLOOKUP", "FILTER", "IF", "CONCAT", "MyUdf"]);
            let count = match name {
                "XLOOKUP" | "IF" => 3,
                "FILTER" => 2,
                _ => 1 + random.below(3),
            };
            let args: Vec<String> = (0..count)
                .map(|_| random_formula(random, depth, parameters))
                .collect();
            format!("{name}({})", args.join(","))
        }
        7 => {
            let name = random.pick(&["x", "итог", "Rate"]);
            let value = random_formula(random, depth, parameters);
            parameters.push(name);
            let body = random_formula(random, depth, parameters);
            parameters.pop();
            format!("LET({name},{value},{body})")
        }
        8 => {
            parameters.extend(["x", "y"]);
            let body = random_formula(random, depth, parameters);
            parameters.truncate(parameters.len() - 2);
            let argument = random_formula(random, depth, parameters);
            format!("LAMBDA(x,[y],{body})({argument})")
        }
        _ => {
            let function = random.pick(&["SUM", "AVERAGE", "HSTACK(SUM,MAX)"]);
            format!("GROUPBY(A1:A3,B1:B3,{function})+BYROW(A1:B3,SUM)")
        }
    }
}

#[test]
fn random_formulas_come_back_through_their_displayed_form() {
    let mut random = Random(0x9E37_79B9_7F4A_7C15);
    let mut checked = 0;
    for _ in 0..3_000 {
        let displayed = random_formula(&mut random, 4, &mut Vec::new());
        let Ok(stored) = to_stored(&displayed) else {
            continue;
        };
        checked += 1;
        let back = to_stored(&to_displayed(&stored));
        assert_eq!(back.as_deref(), Ok(stored.as_str()), "{displayed}");
        assert_eq!(check_stored(&stored), Ok(()), "{displayed}");
        assert!(move_references(&stored, 3, -2).is_ok(), "{stored}");
    }
    assert!(checked > 1_000, "{checked} formulas passed the check");
}

#[test]
fn no_text_makes_a_function_panic() {
    const PIECES: [&str; 30] = [
        "A", "b", "1", "$", ":", "!", "'", "\"", "[", "]", "#", "@", "(", ")", ",", ";", "{", "}",
        "+", " ", ".", "_", "Л", "合", "é", "?", "REF!", "_xl", "\n", "%",
    ];
    let mut random = Random(0xD1B5_4A32_D192_ED03);
    for _ in 0..20_000 {
        let length = 1 + random.below(12);
        let text: String = (0..length).map(|_| random.pick(&PIECES)).collect();
        let _ = tokenize(&text);
        let _ = to_stored(&text);
        let _ = check_stored(&text);
        let _ = move_references(&text, 1, -1);
        let _ = to_displayed(&text);
        let _ = result_extent(&text, |_, name| Some(name.to_owned()));
    }
}

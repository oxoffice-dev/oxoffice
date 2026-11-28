//! A save is atomic, never over the file the workbook was opened from
//! unless asked, and its extension selects the class.

use std::fs;
use std::io::Cursor;
use std::path::PathBuf;

use oxoffice_opc::OpcPackage;
use oxoffice_xlsx::{Error, SaveOptions, Workbook, WorkbookClass};
use oxoffice_xlsx_oxml::namespaces::rel_types::VBA_PROJECT;

use crate::support::corpus_bytes;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("oxoffice-xlsx-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("scratch folder");
    dir
}

/// The main content type of a saved file.
fn main_content_type(path: &PathBuf) -> String {
    let package = OpcPackage::open(path).expect("saved package");
    package
        .content_types
        .content_type_for("/xl/workbook.xml")
        .expect("main content type")
        .to_owned()
}

/// `review` with a VBA project, as Excel saves an `.xlsm`.
fn with_macros() -> Vec<u8> {
    let mut package =
        OpcPackage::from_reader(Cursor::new(corpus_bytes("excel/review.xlsx"))).expect("package");
    package.set_part("/xl/vbaProject.bin", vec![0; 16]);
    package
        .get_or_create_part_rels("/xl/workbook.xml")
        .add(VBA_PROJECT, "vbaProject.bin");
    package.content_types.add_override(
        "/xl/workbook.xml",
        WorkbookClass::MacroEnabledWorkbook.content_type(),
    );
    package
        .content_types
        .add_default("bin", "application/vnd.ms-office.vbaProject");
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).expect("package");
    bytes.into_inner()
}

#[test]
fn the_extension_selects_the_class_and_keeps_a_vba_project_macro_enabled() {
    let dir = scratch("classes");
    let plain = Workbook::from_bytes(&corpus_bytes("excel/review.xlsx")).unwrap();
    let macros = Workbook::from_bytes(&with_macros()).unwrap();
    assert_eq!(macros.class(), WorkbookClass::MacroEnabledWorkbook);
    for (row, (workbook, name, expected)) in [
        (&plain, "copy.xltx", Some(WorkbookClass::Template)),
        (
            &plain,
            "copy.xlsm",
            Some(WorkbookClass::MacroEnabledWorkbook),
        ),
        (&plain, "copy.data", Some(WorkbookClass::Workbook)),
        (
            &macros,
            "copy.xltm",
            Some(WorkbookClass::MacroEnabledTemplate),
        ),
        (&macros, "copy.xlsx", None),
        (&macros, "copy.xltx", None),
    ]
    .into_iter()
    .enumerate()
    {
        let path = dir.join(format!("{row}-{name}"));
        match (workbook.save(&path), expected) {
            (Ok(()), Some(class)) => {
                assert_eq!(main_content_type(&path), class.content_type(), "{name}");
                assert_eq!(Workbook::open(&path).unwrap().class(), class, "{name}");
            }
            (Err(Error::MacroFreeExtension { .. }), None) => assert!(!path.exists(), "{name}"),
            (result, _) => panic!("{name}: {result:?}"),
        }
    }
}

#[test]
fn a_workbook_refuses_to_replace_its_source_unless_asked() {
    let dir = scratch("source");
    let source = dir.join("review.xlsx");
    fs::write(&source, corpus_bytes("libreoffice/review.xlsx")).unwrap();
    let before = fs::read(&source).unwrap();
    let workbook = Workbook::open(&source).unwrap();
    let mut spellings = vec![source.clone(), dir.join(".").join("review.xlsx")];
    #[cfg(unix)]
    {
        let link = dir.join("link.xlsx");
        std::os::unix::fs::symlink(&source, &link).unwrap();
        spellings.push(link);
    }
    for path in &spellings {
        assert!(
            matches!(workbook.save(path), Err(Error::OverwritesSource { .. })),
            "{}",
            path.display()
        );
        assert_eq!(fs::read(&source).unwrap(), before);
    }
    let options = SaveOptions {
        overwrite_source: true,
        ..SaveOptions::default()
    };
    workbook.save_with_options(&source, options).unwrap();
    assert_eq!(fs::read(&source).unwrap(), workbook.to_bytes().unwrap());
    workbook.save(dir.join("copy.xlsx")).unwrap();
}

#[test]
fn a_password_protected_workbook_opens_with_its_password_and_saves_protected() {
    let dir = scratch("encrypted");
    let workbook = Workbook::from_bytes(&corpus_bytes("excel/budget.xlsx")).unwrap();
    let path = dir.join("locked.xlsx");
    workbook.save_encrypted(&path, "secret").unwrap();
    assert!(matches!(
        Workbook::open(&path),
        Err(Error::PasswordProtected)
    ));
    assert!(Workbook::open_encrypted(&path, "wrong").is_err());
    let opened = Workbook::open_encrypted(&path, "secret").unwrap();
    assert_eq!(opened.to_bytes().unwrap(), workbook.to_bytes().unwrap());
    assert!(matches!(
        opened.save_encrypted(&path, "secret"),
        Err(Error::OverwritesSource { .. })
    ));
    assert!(matches!(
        Workbook::from_encrypted_bytes(&workbook.to_bytes().unwrap(), "secret"),
        Err(Error::NotPasswordProtected)
    ));
    // A compound file of no known kind is tried, and the decryption says
    // what it lacks.
    let mut other = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
    other.create_stream("/Contents").unwrap();
    other.flush().unwrap();
    let other = other.into_inner().into_inner();
    assert!(matches!(
        Workbook::from_encrypted_bytes(&other, "secret"),
        Err(Error::Package(_))
    ));
}

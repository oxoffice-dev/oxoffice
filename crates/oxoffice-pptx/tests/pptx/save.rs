//! `Presentation::save` refuses to replace the file the presentation was
//! opened from, whatever the spelling of the path and after any edit, unless
//! asked, and saves anywhere else as before.

use std::fs;
use std::path::{Path, PathBuf};

use oxoffice_pptx::{Error, Presentation};

/// A folder of its own under the system temporary folder, removed on drop.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("oxoffice-pptx-save-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch folder");
        Self(dir)
    }

    /// A copy of the PowerPoint-saved deck of the corpus, named `name`.
    fn deck(&self, name: &str) -> PathBuf {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/corpus/files/powerpoint/deck.pptx");
        let path = self.0.join(name);
        fs::copy(source, &path).expect("copy of the corpus deck");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn assert_refused(prs: &Presentation, path: &Path) {
    let before = fs::read(path).expect("source is readable");
    match prs.save(path) {
        Err(Error::OverwritesSource { .. }) => {}
        other => panic!("{}: expected a refusal, got {other:?}", path.display()),
    }
    assert_eq!(
        fs::read(path).expect("source is readable"),
        before,
        "a refused save left {} as it was",
        path.display()
    );
}

#[test]
fn saving_over_the_opened_file_is_refused() {
    let scratch = Scratch::new("refused");
    let path = scratch.deck("deck.pptx");
    let prs = Presentation::open(&path).unwrap();
    assert_refused(&prs, &path);
    let message = prs.save(&path).unwrap_err().to_string();
    assert!(message.contains("overwrite_source"), "{message}");
}

#[test]
fn the_source_is_still_refused_after_edits() {
    let scratch = Scratch::new("edits");
    let path = scratch.deck("deck.pptx");
    let mut prs = Presentation::open(&path).unwrap();
    let last_layout = prs.layout_count() - 1;

    prs.add_slide(last_layout).unwrap();
    assert_refused(&prs, &path);
    prs.duplicate_slide(0).unwrap();
    assert_refused(&prs, &path);
    prs.move_slide(0, 1).unwrap();
    assert_refused(&prs, &path);
    prs.remove_slide(prs.len() - 1).unwrap();
    assert_refused(&prs, &path);
    prs.set_slide_number_start(Some(3)).unwrap();
    assert_refused(&prs, &path);
    prs.core_properties_mut().title = Some("A title".to_owned());
    assert_refused(&prs, &path);
    prs.replace_text("a", "a");
    assert_refused(&prs, &path);
    let clone = prs.clone();
    assert_refused(&clone, &path);
    prs.save(scratch.0.join("elsewhere.pptx")).unwrap();
    assert_refused(&prs, &path);
}

#[test]
fn saving_elsewhere_leaves_the_source_as_it_was() {
    let scratch = Scratch::new("elsewhere");
    let path = scratch.deck("deck.pptx");
    let before = fs::read(&path).unwrap();
    let mut prs = Presentation::open(&path).unwrap();
    let slides = prs.len();
    prs.duplicate_slide(0).unwrap();
    let copy = scratch.0.join("copy.pptx");
    prs.save(&copy).unwrap();
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(Presentation::open(&copy).unwrap().len(), slides + 1);
}

/// A blank deck of `slides` slides carrying a package signature graph whose
/// signature references nothing, which a build without the
/// `digital-signatures` feature reads as signed and not invalidated. Its
/// app.xml keeps the title lists a slide edit drops, and it has no
/// thumbnail, so app.xml is what an edit leaves changed.
#[cfg(not(feature = "digital-signatures"))]
fn signed_blank_deck(slides: usize) -> Presentation {
    use oxoffice_opc::OpcPackage;
    use oxoffice_opc::relationship::rel_types;

    let mut prs = crate::support::new_deck();
    let blank = (0..prs.layout_count())
        .find(|&index| prs.layout_name(index) == Some("Blank"))
        .expect("Blank layout");
    for _ in 0..slides {
        prs.add_slide(blank).unwrap();
    }
    let mut package =
        OpcPackage::from_reader(std::io::Cursor::new(prs.to_bytes().unwrap())).unwrap();
    package.set_part(
        "/docProps/app.xml",
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\" \
             xmlns:vt=\"http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes\">\
             <Slides>{slides}</Slides><HeadingPairs><vt:vector size=\"2\" baseType=\"variant\">\
             <vt:variant><vt:lpstr>Theme</vt:lpstr></vt:variant><vt:variant><vt:i4>1</vt:i4></vt:variant>\
             </vt:vector></HeadingPairs><TitlesOfParts><vt:vector size=\"1\" baseType=\"lpstr\">\
             <vt:lpstr>Office Theme</vt:lpstr></vt:vector></TitlesOfParts></Properties>"
        )
        .into_bytes(),
    );
    package.remove_part("/docProps/thumbnail.jpeg");
    package
        .package_rels
        .items
        .retain(|relationship| relationship.rel_type != rel_types::THUMBNAIL);
    package.set_part("/_xmlsignatures/origin.sigs", Vec::new());
    package.set_part(
        "/_xmlsignatures/sig1.xml",
        br#"<Signature xmlns="http://www.w3.org/2000/09/xmldsig#"><SignedInfo/></Signature>"#
            .to_vec(),
    );
    package.content_types.add_override(
        "/_xmlsignatures/origin.sigs",
        "application/vnd.openxmlformats-package.digital-signature-origin",
    );
    package.content_types.add_override(
        "/_xmlsignatures/sig1.xml",
        "application/vnd.openxmlformats-package.digital-signature-xmlsignature+xml",
    );
    package.package_rels.add(
        rel_types::DIGITAL_SIGNATURE_ORIGIN,
        "_xmlsignatures/origin.sigs",
    );
    package
        .get_or_create_part_rels("/_xmlsignatures/origin.sigs")
        .add(rel_types::DIGITAL_SIGNATURE, "sig1.xml");
    let mut bytes = Vec::new();
    package
        .write_to(&mut std::io::Cursor::new(&mut bytes))
        .unwrap();
    Presentation::from_bytes(&bytes).unwrap()
}

/// Whether a saved package marks its package signature invalidated.
#[cfg(not(feature = "digital-signatures"))]
fn marks_signature_invalidated(bytes: &[u8]) -> bool {
    oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(bytes))
        .unwrap()
        .package_rels
        .items
        .iter()
        .any(|relationship| {
            relationship
                .rel_type
                .ends_with("/invalidated-package-signature")
        })
}

#[cfg(not(feature = "digital-signatures"))]
#[test]
fn a_signed_deck_edited_in_place_and_back_still_marks_its_signature_invalidated() {
    let unchanged = signed_blank_deck(2);
    let saved = unchanged.to_bytes().unwrap();
    assert!(!marks_signature_invalidated(&saved));
    assert!(
        oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(&saved))
            .unwrap()
            .get_part("/docProps/app.xml")
            .is_some_and(|xml| String::from_utf8_lossy(xml).contains("TitlesOfParts")),
        "the fixture's app.xml keeps its title lists until an edit"
    );
    let blank = (0..unchanged.layout_count())
        .find(|&index| unchanged.layout_name(index) == Some("Blank"))
        .unwrap();
    // Each edit writes the package in place and drops app.xml's title
    // lists; the removal that undoes it in the slide list leaves the slide
    // list as it was, but not app.xml, which the signature covers.
    type Edit = fn(&mut Presentation, usize);
    let edits: [(&str, Edit); 3] = [
        ("add_slide", |prs, layout| {
            prs.add_slide(layout).unwrap();
        }),
        ("add_slide_at", |prs, layout| {
            prs.add_slide_at(layout, 1).unwrap();
        }),
        ("duplicate_slide", |prs, _| {
            prs.duplicate_slide(0).unwrap();
        }),
    ];
    for (name, edit) in edits {
        let mut prs = unchanged.clone();
        edit(&mut prs, blank);
        let added = if name == "add_slide" { 2 } else { 1 };
        prs.remove_slide(added).unwrap();
        assert!(
            marks_signature_invalidated(&prs.to_bytes().unwrap()),
            "{name} then remove_slide"
        );
    }
}

/// A throwaway RSA key and self-signed certificate ("oxoffice test signer"),
/// made for the docx signing test and reused here.
#[cfg(all(feature = "digital-signatures", not(target_arch = "wasm32")))]
const TEST_SIGNING_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIIEvAIBADANBgkqhkiG9w0BAQEFAASCBKYwggSiAgEAAoIBAQCfO5vleZ3XkHcn
hDZ0v02CxdPfzONtzvdXNS3oSDMYVYzJ/8bNtdAC5YGZcXBGIiz1yo+2J1Q1dxYC
YJoeGtNLretyz2xt3iYQOeiWGQV9ayaH9Snun+60kXSKorKbT5JnywxlTD6dt/zi
0vLDrjlGGGISgtmPAvPE8g8WNTQfwVeDQwJCbDcCUBrsg8p4PoC5ubjqbQXlHtPq
wNpyJqJelq50SFzAKglGiNEnDs+nLCE8dYaOscmE8groL8zGqYoUEBoY9hsM6Smq
Tt6SPH448jvMHlQoso/6+iPPI9xi1EHUrVHFntdZkQ1omCc8CMxLvufq18y2Au9B
gfplrs55AgMBAAECggEAAjI0ZNpz67mPeSUsypiqWSHBGbK11sl+Vg6y1MOtJ7pj
VBRh4auupCGV81+yBpeZUuUBrSeW9NtQ+4SpT327eQ1bpFRZyJyiHgJsc6iB78BC
8mhWVRcR193zM2fJypRS/rk5USrXaL21QEwcQshj/BRhQuPY3Az0f3x+qoXek4sF
pbgi6oiBeoZsIUhgz/dwP4S6nAvS+/+0S395Hzrl6LkZ79ghCEWWL5ANZeA0Eg7Q
8/1EeKDBg9M638BftMNMI1aqOrhXwAruFpQ2y8T94JDrVBduTr2+NJO2hZmbd55g
qUwTrGvXytgEIT7gZqwZ/2Q+AYpkwB2kKtabH3xuQQKBgQDV9NkxqNLHqXrRV1mZ
kCs17myEdDuAlEMQrp2zpt+HFqPPZgePcniv9iV+e6/lbMfQTRRipcQWULeviGc/
lYZLDsXsRBWeiLimqL5CBVe05W46YCCMMwISh3xhZqrqNb3blUCs6xGTqoS4ojEd
Ns42JzXAWB1DhJq5JUPc6QzAOQKBgQC+hd8jSf/1hB+yPz02A171afwhg48nQ7Sz
WjmQKsEaCwRZvjAgbDZDo/FWBIXEcyPF7TproX3SMbc899cmTqCi/vkWLe5EQgFj
dZn+TZUNbOwDr0Eui24o436UwXB3MUYLJDPmqy2CXX+KViw6eTVwuTtVBX+mHV06
Hd1VgQIAQQKBgF8hDYzh5k32EfCQKPt5OW0yRDekQmyJ/0Okj+O3130F2PG3CLeE
CxsZk1QohG26ryDzlJPD5mqRmwpF2LpRrjMjL2c8Ymxpl8gqEOXj4DpDJH56DR6o
80vvx6K69lVKbc77DpWTsu34DCjXnITDwG4dgu2xf9lsVSrq4cCQMiYpAoGAWFOa
CjMxXfJd+3YD+7l+WYpEIIlsYYirCocmrqaTlW5MGwroJu/kDGHB7d304gl7KTR8
5A8fVMZFrBc+gHztTgOlGNo8Xxq2qBSKSvJOysWXeXHTZy+u53foZDB14lxOm7Wx
9huf4pNUIJ8orAcrdlH5psYA0tNEOEBGL3C6q8ECgYAjgD2wQLN19HdHWW8xOSLB
7KcBE9mCBbEHCv/hVkmZkfpgul2QktKpW6VuXJ1OKnL8myU3j4lu50y3ETqYArOo
LUdwV9LlqCIUoWJcwCrtyI1x+C5tGDuFQ6KdrBmioxpVvDk4Pkl4jLhk8oYNs3mJ
1ehN38Jf8t8ftFypHJ3TPA==
-----END PRIVATE KEY-----";
#[cfg(all(feature = "digital-signatures", not(target_arch = "wasm32")))]
const TEST_SIGNING_CERTIFICATE: &str = "-----BEGIN CERTIFICATE-----
MIIDITCCAgmgAwIBAgIUVjb4pGFt/vPS9mEbcE7WAhlyefwwDQYJKoZIhvcNAQEL
BQAwHzEdMBsGA1UEAwwUb3hvZmZpY2UgdGVzdCBzaWduZXIwIBcNMjYxMTIxMTYw
MjE3WhgPMjEyNjEwMjgxNjAyMTdaMB8xHTAbBgNVBAMMFG94b2ZmaWNlIHRlc3Qg
c2lnbmVyMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAnzub5Xmd15B3
J4Q2dL9NgsXT38zjbc73VzUt6EgzGFWMyf/GzbXQAuWBmXFwRiIs9cqPtidUNXcW
AmCaHhrTS63rcs9sbd4mEDnolhkFfWsmh/Up7p/utJF0iqKym0+SZ8sMZUw+nbf8
4tLyw645RhhiEoLZjwLzxPIPFjU0H8FXg0MCQmw3AlAa7IPKeD6Aubm46m0F5R7T
6sDaciaiXpaudEhcwCoJRojRJw7PpywhPHWGjrHJhPIK6C/MxqmKFBAaGPYbDOkp
qk7ekjx+OPI7zB5UKLKP+vojzyPcYtRB1K1RxZ7XWZENaJgnPAjMS77n6tfMtgLv
QYH6Za7OeQIDAQABo1MwUTAdBgNVHQ4EFgQU6MAk9nMeKGifrYqPcDl4AHV+D80w
HwYDVR0jBBgwFoAU6MAk9nMeKGifrYqPcDl4AHV+D80wDwYDVR0TAQH/BAUwAwEB
/zANBgkqhkiG9w0BAQsFAAOCAQEAFf6fMW73k8zeBx8Wy7rrfqyJWMLhuVCc6YET
o3TUdpVBe+xG4fXP9weGvCQYQEVEXnbmJrSRxlEnDnXgkTQKPD/i9EkpJV87VQxB
AhlY/qnzQ1fSytkgbuC7vFvT+AZj//i+nfuswvIPjBVr/qIzIYqZf2L9tBX/Cs41
LszcMvbYTZE24iw85TAXBor+erenQduzLnVo5HBcdJLVpM+UYoeGK+PVeP+JfGeT
FN2AZ5FDJqUpH3Hd7k0ApiMkCxIHRAmQ9rFQ65Uk7SksvAdGVYslsa1WD1dvZhmH
zN38JSIrXn+Dpw+iPeqw+r+h0PsTLRkVqq+uAXV/EpW8/DI0rg==
-----END CERTIFICATE-----";

#[cfg(all(feature = "digital-signatures", not(target_arch = "wasm32")))]
#[test]
fn a_deck_signed_in_session_and_edited_in_place_and_back_marks_its_signature_invalidated() {
    let mut prs = crate::support::new_deck();
    let blank = (0..prs.layout_count())
        .find(|&index| prs.layout_name(index) == Some("Blank"))
        .unwrap();
    prs.add_slide(blank).unwrap();
    prs.sign(
        TEST_SIGNING_KEY.as_bytes(),
        TEST_SIGNING_CERTIFICATE.as_bytes(),
    )
    .unwrap();
    // The commit of the removal compares with the package as signed, not
    // with the one the insert wrote in place.
    prs.add_slide_at(blank, 1).unwrap();
    prs.remove_slide(1).unwrap();
    let bytes = prs.to_bytes().unwrap();
    let package = oxoffice_opc::OpcPackage::from_reader(std::io::Cursor::new(bytes)).unwrap();
    assert!(
        package
            .package_rels
            .items
            .iter()
            .any(|relationship| relationship
                .rel_type
                .ends_with("/invalidated-package-signature")),
        "the signature is marked invalidated"
    );
}

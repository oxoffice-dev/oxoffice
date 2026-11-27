//! A workbook signed as a package keeps its signature through a save that
//! changes nothing, and a save that changes anything marks it invalidated,
//! as docx and pptx do: the signature parts stay as evidence, and the
//! package says by a relationship that they no longer hold.

#![cfg(feature = "digital-signatures")]

use std::io::Cursor;

use oxoffice_opc::OpcPackage;
use oxoffice_xlsx::Workbook;

use crate::support::{PartsDiff, package_parts, parts_diff};

/// A throwaway RSA key and self-signed certificate ("oxoffice test signer"),
/// made for the docx signing test and reused by pptx and xlsx.
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

fn marked_invalidated(bytes: &[u8]) -> bool {
    let package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
    package.package_rels.items.iter().any(|relationship| {
        relationship
            .rel_type
            .ends_with("/invalidated-package-signature")
    })
}

fn signed_workbook() -> Vec<u8> {
    let bytes = Workbook::new().to_bytes().unwrap();
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
    package
        .sign(
            TEST_SIGNING_KEY.as_bytes(),
            TEST_SIGNING_CERTIFICATE.as_bytes(),
        )
        .unwrap();
    let mut signed = Cursor::new(Vec::new());
    package.write_to(&mut signed).unwrap();
    signed.into_inner()
}

#[test]
fn an_edit_marks_the_package_signature_invalidated_and_a_save_without_one_leaves_it() {
    let signed = signed_workbook();
    let verifies = |bytes: &[u8]| {
        let package = OpcPackage::from_reader(Cursor::new(bytes)).unwrap();
        let reports = package.verify_signatures().unwrap();
        !reports.is_empty()
            && reports
                .iter()
                .all(|report| report.cryptographically_valid && report.coverage_complete)
    };
    assert!(verifies(&signed));

    // Opened and saved, or edited with a call that changes nothing: the
    // package is the one that was signed.
    let mut workbook = Workbook::from_bytes(&signed).unwrap();
    assert_eq!(
        parts_diff(&signed, &workbook.to_bytes().unwrap()),
        PartsDiff::default()
    );
    workbook
        .sheet_at_mut(0)
        .unwrap()
        .freeze_panes(None)
        .unwrap();
    assert_eq!(workbook.to_bytes().unwrap(), signed);

    // An edit: the signature parts stay, the package says they no longer hold.
    workbook
        .sheet_at_mut(0)
        .unwrap()
        .set_column_width("A", 20.0)
        .unwrap();
    let edited = workbook.to_bytes().unwrap();
    assert!(marked_invalidated(&edited));
    assert!(!verifies(&edited));
    let (before, after) = (package_parts(&signed), package_parts(&edited));
    assert!(
        before
            .keys()
            .filter(|name| name.starts_with("_xmlsignatures/"))
            .all(|name| after.get(name) == before.get(name)),
        "the signature parts are kept as they were"
    );

    // The mark is carried by every later save, and written once.
    let again = Workbook::from_bytes(&edited).unwrap().to_bytes().unwrap();
    assert!(marked_invalidated(&again));
    assert_eq!(parts_diff(&edited, &again), PartsDiff::default());

    // A class change (the extension) changes the package as well.
    let path =
        std::env::temp_dir().join(format!("oxoffice-xlsx-signed-{}.xltx", std::process::id()));
    Workbook::from_bytes(&signed).unwrap().save(&path).unwrap();
    assert!(marked_invalidated(&std::fs::read(&path).unwrap()));
    let _ = std::fs::remove_file(path);
}

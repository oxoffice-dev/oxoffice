//! `oxoffice docx verify-signatures` judges the package as opened: the
//! signatures cover the bytes on disk.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;

use oxoffice_docx::Document;
use oxoffice_opc::OpcPackage;

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

/// A scratch folder of its own for `name`.
fn scratch(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("oxoffice-cli-verify-{name}-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("scratch folder");
    dir
}

/// A Word package signed as it is, its main part written with whitespace
/// between the body's blocks (as hand-written or other producers' files are).
fn signed_package() -> OpcPackage {
    let mut doc = Document::new();
    doc.add_paragraph("signed");
    let bytes = doc.to_bytes().expect("a document");
    let mut package = OpcPackage::from_reader(Cursor::new(bytes)).expect("a package");
    let main = package
        .get_part("/word/document.xml")
        .expect("a main part")
        .to_vec();
    let main = String::from_utf8(main)
        .expect("UTF-8")
        .replacen("<w:body>", "<w:body>\n  ", 1);
    package.set_part("/word/document.xml", main.into_bytes());
    package
        .sign(
            TEST_SIGNING_KEY.as_bytes(),
            TEST_SIGNING_CERTIFICATE.as_bytes(),
        )
        .expect("signed");
    package
}

/// Save `package` in `dir` and run `oxoffice docx verify-signatures --json`
/// on it: the exit status and the reported status.
fn verify(dir: &Path, package: &OpcPackage) -> (bool, String) {
    let path = dir.join("signed.docx");
    let mut bytes = Cursor::new(Vec::new());
    package.write_to(&mut bytes).expect("written");
    fs::write(&path, bytes.into_inner()).expect("saved");
    let run = Command::new(env!("CARGO_BIN_EXE_oxoffice"))
        .args(["docx", "verify-signatures"])
        .arg(&path)
        .arg("--json")
        .output()
        .expect("the CLI runs");
    let report: serde_json::Value = serde_json::from_slice(&run.stdout)
        .unwrap_or_else(|_| panic!("JSON on stdout: {}", String::from_utf8_lossy(&run.stderr)));
    (
        run.status.success(),
        report["status"].as_str().unwrap_or_default().to_owned(),
    )
}

#[test]
fn a_signed_word_file_is_valid_as_opened() {
    let dir = scratch("valid");
    assert_eq!(verify(&dir, &signed_package()), (true, "valid".to_owned()));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_signed_word_file_the_library_would_rewrite_is_valid_as_opened() {
    let dir = scratch("rewritten");
    let mut doc = Document::new();
    doc.add_paragraph("signed");
    let mut package = OpcPackage::from_reader(Cursor::new(doc.to_bytes().expect("a document")))
        .expect("a package");
    // An `AppVersion` outside Word's `XX.YYYY` form, as rdocx 0.14.0 wrote
    // it: opening repairs it, so writing the file back changes this signed
    // part. The signature still covers the bytes on disk.
    let app = String::from_utf8(
        package
            .get_part("/docProps/app.xml")
            .expect("an app properties part")
            .to_vec(),
    )
    .expect("UTF-8");
    let start = app.find("<AppVersion>").expect("an AppVersion") + "<AppVersion>".len();
    let end = app.find("</AppVersion>").expect("its end");
    let app = format!("{}0.14.0{}", &app[..start], &app[end..]);
    package.set_part("/docProps/app.xml", app.into_bytes());
    package
        .sign(
            TEST_SIGNING_KEY.as_bytes(),
            TEST_SIGNING_CERTIFICATE.as_bytes(),
        )
        .expect("signed");
    assert_eq!(verify(&dir, &package), (true, "valid".to_owned()));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_word_file_changed_after_signing_is_invalid() {
    let dir = scratch("changed");
    let mut package = signed_package();
    let main = package
        .get_part("/word/document.xml")
        .expect("a main part")
        .to_vec();
    let main = String::from_utf8(main)
        .expect("UTF-8")
        .replacen(">signed<", ">changed<", 1);
    package.set_part("/word/document.xml", main.into_bytes());
    let (passed, status) = verify(&dir, &package);
    assert!(!passed, "a changed signed part fails the check");
    assert_eq!(status, "invalid");
    fs::remove_dir_all(&dir).ok();
}

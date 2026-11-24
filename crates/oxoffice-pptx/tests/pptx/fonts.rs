//! Embedded fonts: the licence and face checks a deck applies before it
//! embeds a font, the same as a Word document's.

use oxoffice_pptx::{EmbeddedFont, EmbeddedFontKind, FontEmbeddingLicense};

use crate::support::new_deck;

/// A 4 KB subset of Noto Sans Symbols 2, bundled with the layout crate under
/// the SIL Open Font License 1.1.
const NOTO_SYMBOLS_2: &[u8] =
    include_bytes!("../../../oxoffice-layout/fonts/NotoSansSymbols2-bullets-subset.ttf");

#[test]
fn a_license_identity_with_a_control_character_is_refused() {
    let mut prs = new_deck();
    let before = prs.to_bytes().expect("saves");
    for identity in ["OFL\u{1}1.1", "OFL\t1.1", "OFL 1.1\u{1f}"] {
        let font = EmbeddedFont::new(
            EmbeddedFontKind::Regular,
            NOTO_SYMBOLS_2.to_vec(),
            FontEmbeddingLicense::new(true, identity),
        );
        assert!(
            prs.embed_font("Noto Sans Symbols 2", font).is_err(),
            "{identity:?} is refused"
        );
    }
    assert_eq!(prs.to_bytes().expect("saves"), before);

    let font = EmbeddedFont::new(
        EmbeddedFontKind::Regular,
        NOTO_SYMBOLS_2.to_vec(),
        FontEmbeddingLicense::new(true, "SIL Open Font License 1.1"),
    );
    prs.embed_font("Noto Sans Symbols 2", font)
        .expect("a printable identity is accepted");
}

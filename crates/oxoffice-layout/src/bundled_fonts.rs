//! Bundled fallback fonts for standalone operation.
//!
//! The following fonts are embedded in the binary:
//!
//! - **Carlito** — metric-compatible with Calibri (Word's default font),
//!   licensed under the SIL Open Font License 1.1
//! - **Caladea** — metric-compatible with Cambria, licensed under the Apache
//!   License 2.0
//! - **Gelasio** upright and italic variable faces, approximate substitutes for
//!   Georgia, licensed under the SIL Open Font License 1.1
//! - **Selawik** Regular and Bold, bounded Segoe UI substitutes without genuine
//!   italic faces, licensed under the SIL Open Font License 1.1
//! - **Noto Sans Symbols 2** approved monochrome bullet and checkbox subset,
//!   licensed under the SIL Open Font License 1.1
//! - **Liberation Sans** — metric-compatible with Arial, licensed under the
//!   SIL Open Font License 1.1
//! - **Liberation Serif** — metric-compatible with Times New Roman, licensed
//!   under the SIL Open Font License 1.1
//! - **Liberation Mono** — metric-compatible with Courier New, licensed under
//!   the SIL Open Font License 1.1
//! - **Noto Sans Arabic**, **Noto Sans Devanagari**, **Noto Sans Thai**,
//!   **Noto Sans SC**, **Noto Sans Hebrew**, **Noto Sans KR**, and **Noto Sans
//!   JP** — deterministic complex-script fallbacks licensed under the SIL Open
//!   Font License 1.1
//!
//! The Noto Sans SC, Hebrew, KR and JP faces are deterministic subsets, not
//! the full families. Each ships only its approved fixture repertoire, which
//! its `fonts/SUBSET-*.md` record names and reproduces.

/// Returns bundled font data: `(family_name, font_bytes)` pairs.
///
/// Returns Carlito, Caladea, Liberation Sans, Liberation Serif, and Liberation
/// Mono, each with Regular, Bold, Italic, and BoldItalic variants. Gelasio keeps
/// its original upright and italic variable faces. Selawik supplies Regular and
/// Bold. Noto Sans Symbols 2 supplies only the approved subset repertoire.
pub fn bundled_font_data() -> Vec<(&'static str, &'static [u8])> {
    vec![
        // Carlito — metric-compatible replacement for Calibri
        (
            "Carlito",
            include_bytes!("../fonts/Carlito-Regular.ttf").as_slice(),
        ),
        (
            "Carlito",
            include_bytes!("../fonts/Carlito-Bold.ttf").as_slice(),
        ),
        (
            "Carlito",
            include_bytes!("../fonts/Carlito-Italic.ttf").as_slice(),
        ),
        (
            "Carlito",
            include_bytes!("../fonts/Carlito-BoldItalic.ttf").as_slice(),
        ),
        // Caladea — metric-compatible replacement for Cambria
        (
            "Caladea",
            include_bytes!("../fonts/Caladea-Regular.ttf").as_slice(),
        ),
        (
            "Caladea",
            include_bytes!("../fonts/Caladea-Bold.ttf").as_slice(),
        ),
        (
            "Caladea",
            include_bytes!("../fonts/Caladea-Italic.ttf").as_slice(),
        ),
        (
            "Caladea",
            include_bytes!("../fonts/Caladea-BoldItalic.ttf").as_slice(),
        ),
        // Original variable faces preserve the full approved wght range.
        (
            "Gelasio",
            include_bytes!("../fonts/Gelasio-Regular.ttf").as_slice(),
        ),
        (
            "Gelasio",
            include_bytes!("../fonts/Gelasio-Italic.ttf").as_slice(),
        ),
        // Selawik has no genuine italic. Style synthesis remains provider-owned.
        (
            "Selawik",
            include_bytes!("../fonts/Selawik-Regular.ttf").as_slice(),
        ),
        (
            "Selawik",
            include_bytes!("../fonts/Selawik-Bold.ttf").as_slice(),
        ),
        (
            "Noto Sans Symbols 2",
            include_bytes!("../fonts/NotoSansSymbols2-bullets-subset.ttf").as_slice(),
        ),
        // Liberation Sans — metric-compatible replacement for Arial
        (
            "Liberation Sans",
            include_bytes!("../fonts/LiberationSans-Regular.ttf").as_slice(),
        ),
        (
            "Liberation Sans",
            include_bytes!("../fonts/LiberationSans-Bold.ttf").as_slice(),
        ),
        (
            "Liberation Sans",
            include_bytes!("../fonts/LiberationSans-Italic.ttf").as_slice(),
        ),
        (
            "Liberation Sans",
            include_bytes!("../fonts/LiberationSans-BoldItalic.ttf").as_slice(),
        ),
        // Liberation Serif — metric-compatible replacement for Times New Roman
        (
            "Liberation Serif",
            include_bytes!("../fonts/LiberationSerif-Regular.ttf").as_slice(),
        ),
        (
            "Liberation Serif",
            include_bytes!("../fonts/LiberationSerif-Bold.ttf").as_slice(),
        ),
        (
            "Liberation Serif",
            include_bytes!("../fonts/LiberationSerif-Italic.ttf").as_slice(),
        ),
        (
            "Liberation Serif",
            include_bytes!("../fonts/LiberationSerif-BoldItalic.ttf").as_slice(),
        ),
        // Liberation Mono — metric-compatible replacement for Courier New
        (
            "Liberation Mono",
            include_bytes!("../fonts/LiberationMono-Regular.ttf").as_slice(),
        ),
        (
            "Liberation Mono",
            include_bytes!("../fonts/LiberationMono-Bold.ttf").as_slice(),
        ),
        (
            "Liberation Mono",
            include_bytes!("../fonts/LiberationMono-Italic.ttf").as_slice(),
        ),
        (
            "Liberation Mono",
            include_bytes!("../fonts/LiberationMono-BoldItalic.ttf").as_slice(),
        ),
        (
            "Noto Sans Arabic",
            include_bytes!("../fonts/NotoSansArabic.ttf").as_slice(),
        ),
        (
            "Noto Sans Devanagari",
            include_bytes!("../fonts/NotoSansDevanagari.ttf").as_slice(),
        ),
        (
            "Noto Sans Thai",
            include_bytes!("../fonts/NotoSansThai.ttf").as_slice(),
        ),
        (
            "Noto Sans SC",
            include_bytes!("../fonts/NotoSansSC-FX058-subset.ttf").as_slice(),
        ),
        (
            "Noto Sans Hebrew",
            include_bytes!("../fonts/NotoSansHebrew-F266a-subset.ttf").as_slice(),
        ),
        (
            "Noto Sans KR",
            include_bytes!("../fonts/NotoSansKR-F266a-subset.ttf").as_slice(),
        ),
        (
            "Noto Sans JP",
            include_bytes!("../fonts/NotoSansJP-F266a-subset.ttf").as_slice(),
        ),
    ]
}

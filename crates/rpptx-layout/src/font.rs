//! Theme typeface resolution.

use oxml_drawing::theme::CT_FontCollection;

use crate::ResolveCtx;

impl ResolveCtx<'_> {
    /// Resolves a DrawingML theme typeface token to a concrete font face.
    ///
    /// A supplied ISO 15924 script selects the first matching supplemental
    /// face in document order. Ordinary faces and unknown tokens pass through
    /// unchanged.
    pub fn resolve_typeface(&self, typeface: &str, script: Option<&str>) -> String {
        let scheme = &self.theme.theme_elements.font_scheme;
        let (collection, base) = match typeface {
            "+mj-lt" => (&scheme.major_font, &scheme.major_font.latin.typeface),
            "+mn-lt" => (&scheme.minor_font, &scheme.minor_font.latin.typeface),
            "+mj-ea" => (&scheme.major_font, &scheme.major_font.east_asian.typeface),
            "+mn-ea" => (&scheme.minor_font, &scheme.minor_font.east_asian.typeface),
            "+mj-cs" => (
                &scheme.major_font,
                &scheme.major_font.complex_script.typeface,
            ),
            "+mn-cs" => (
                &scheme.minor_font,
                &scheme.minor_font.complex_script.typeface,
            ),
            _ => return typeface.to_owned(),
        };

        supplemental_typeface(collection, script)
            .unwrap_or(base)
            .clone()
    }
}

fn supplemental_typeface<'a>(
    collection: &'a CT_FontCollection,
    script: Option<&str>,
) -> Option<&'a String> {
    let script = script?;
    collection
        .supplemental_fonts
        .iter()
        .find(|font| font.script == script)
        .map(|font| &font.typeface)
}

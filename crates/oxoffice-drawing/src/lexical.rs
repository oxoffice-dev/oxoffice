//! The lexical forms of DrawingML values that Strict parts write otherwise
//! than Transitional ones.
//!
//! A percentage (`ST_Percentage` and its positive and fixed kinds) is an
//! integer of thousandths of a percent in a Transitional part (`50000`) and
//! a decimal followed by `%` in a Strict one (`50%`, `12.5%`): both read as
//! the same thousandths. Angles are integers of 60,000ths of a degree in
//! both. The writers write the Transitional form: the facades that write
//! DrawingML open Transitional packages only.

/// A percentage in thousandths of a percent, from either form; `None` for
/// any other text or a value past `i32`.
pub(crate) fn parse_percentage(text: &str) -> Option<i32> {
    let Some(percent) = text.strip_suffix('%') else {
        return text.parse().ok();
    };
    let digits = percent.strip_prefix('-').unwrap_or(percent);
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, "0"));
    let decimal = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    if !decimal(whole) || !decimal(fraction) {
        return None;
    }
    let thousandths = (percent.parse::<f64>().ok()? * 1000.0).round();
    (thousandths.abs() <= f64::from(i32::MAX)).then_some(thousandths as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_percentage_reads_in_both_forms() {
        for (text, thousandths) in [
            ("50000", Some(50_000)),
            ("-25000", Some(-25_000)),
            ("50%", Some(50_000)),
            ("12.5%", Some(12_500)),
            ("-80%", Some(-80_000)),
            ("0.0005%", Some(1)),
            ("350%", Some(350_000)),
            ("%", None),
            ("5.%", None),
            ("+5%", None),
            ("1e3%", None),
            ("50 %", None),
            ("9999999999%", None),
            ("half", None),
        ] {
            assert_eq!(parse_percentage(text), thousandths, "{text}");
        }
    }

    /// `xml` with the percentage attributes of DrawingML written as a Strict
    /// part writes them: `pos="35000"` is `pos="35%"`.
    fn strict(xml: &str) -> String {
        const PERCENTAGES: [&str; 12] = [
            "pos", "val", "amt", "sx", "sy", "l", "t", "r", "b", "lim", "d", "sp",
        ];
        let mut out = String::new();
        let mut rest = xml;
        while let Some(at) = rest.find("=\"") {
            let name = rest[..at].rsplit([' ', '<']).next().unwrap_or("");
            let end = at + 2 + rest[at + 2..].find('"').expect("a closing quote");
            let value = &rest[at + 2..end];
            out.push_str(&rest[..at + 2]);
            match value.parse::<i32>() {
                Ok(thousandths) if PERCENTAGES.contains(&name) => {
                    out.push_str(&format!("{}%", f64::from(thousandths) / 1000.0));
                }
                _ => out.push_str(value),
            }
            rest = &rest[end..];
        }
        out.push_str(rest);
        out
    }

    /// What a case reads its fragment with, as text.
    type Reader = dyn Fn(&str) -> String;

    #[test]
    fn every_percentage_reader_takes_the_strict_form() {
        let fill = |xml: &str| format!("{:?}", crate::fill::Fill::from_xml(xml.as_bytes()));
        let effects = |xml: &str| {
            format!(
                "{:?}",
                crate::effect::CT_EffectList::from_xml(xml.as_bytes())
            )
        };
        let line = |xml: &str| {
            format!(
                "{:?}",
                crate::line::CT_LineProperties::from_xml(xml.as_bytes())
            )
        };
        let cases: [(&Reader, &str); 4] = [
            (
                &fill,
                concat!(
                    r#"<a:gradFill><a:gsLst><a:gs pos="35000"><a:srgbClr val="4472C4">"#,
                    r#"<a:tint val="50000"/><a:shade val="12500"/><a:satMod val="300000"/>"#,
                    r#"<a:lumMod val="75000"/><a:lumOff val="-25000"/><a:alpha val="40000"/>"#,
                    r#"<a:hueMod val="90000"/></a:srgbClr></a:gs></a:gsLst><a:path path="circle">"#,
                    r#"<a:fillToRect l="50000" t="-80000" r="50000" b="180000"/></a:path></a:gradFill>"#
                ),
            ),
            (
                &fill,
                concat!(
                    r#"<a:blipFill><a:blip r:embed="rId1"><a:alphaModFix amt="40000"/></a:blip>"#,
                    r#"<a:srcRect l="10000" r="5000"/>"#,
                    r#"<a:tile tx="0" ty="0" sx="50000" sy="75000" flip="none" algn="tl"/></a:blipFill>"#
                ),
            ),
            (
                &effects,
                concat!(
                    r#"<a:effectLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">"#,
                    r#"<a:outerShdw blurRad="40000" dist="23000" dir="5400000" sx="102000" sy="98000">"#,
                    r#"<a:srgbClr val="0D0D0D"><a:alpha val="35000"/></a:srgbClr></a:outerShdw></a:effectLst>"#
                ),
            ),
            (
                &line,
                concat!(
                    r#"<a:ln w="9525"><a:custDash><a:ds d="300000" sp="100000"/></a:custDash>"#,
                    r#"<a:miter lim="800000"/></a:ln>"#
                ),
            ),
        ];
        for (read, transitional) in cases {
            let strict = strict(transitional);
            assert_ne!(strict, transitional);
            let model = read(transitional);
            assert!(model.starts_with("Ok("), "{transitional}: {model}");
            assert_eq!(read(&strict), model, "{strict}");
        }
    }
}

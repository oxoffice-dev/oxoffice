//! Embedded fonts, font definitions and embedding licences, and the fonts a caller passes.

use super::*;

pub(super) fn collect_caller_fonts<'py>(
    py: Python<'py>,
    fonts: Option<Vec<(String, Bound<'py, PyBytes>)>>,
    font_dir: Option<PathBuf>,
    font_dirs: Option<Bound<'py, PyAny>>,
) -> PyResult<Option<Vec<oxoffice_layout::FontFile>>> {
    oxoffice_py_support::fonts::collect_caller_fonts(py, fonts, font_dir, font_dirs, |error| {
        docx_to_pyerr(py, oxoffice_docx::Error::Layout(error))
    })
}

/// Caller authorization is required independently of the font's own fsType bits.
#[pyclass(
    name = "FontEmbeddingLicense",
    frozen,
    get_all,
    eq,
    skip_from_py_object
)]
#[derive(Clone, PartialEq, Eq)]
pub struct PyFontEmbeddingLicense {
    pub authorized: bool,
    pub identity: String,
}
#[pymethods]
impl PyFontEmbeddingLicense {
    #[new]
    #[pyo3(signature = (*, authorized, identity))]
    fn new(authorized: bool, identity: String) -> Self {
        Self {
            authorized,
            identity,
        }
    }
}

pub(super) use oxoffice_py_support::fonts::checked_font_input;

fn font_kind_name(kind: oxoffice_docx::EmbeddedFontKind) -> &'static str {
    match kind {
        oxoffice_docx::EmbeddedFontKind::Regular => "regular",
        oxoffice_docx::EmbeddedFontKind::Bold => "bold",
        oxoffice_docx::EmbeddedFontKind::Italic => "italic",
        oxoffice_docx::EmbeddedFontKind::BoldItalic => "bold_italic",
    }
}
pub(super) fn checked_font_kind(kind: &str) -> PyResult<oxoffice_docx::EmbeddedFontKind> {
    match kind {
        "regular" => Ok(oxoffice_docx::EmbeddedFontKind::Regular),
        "bold" => Ok(oxoffice_docx::EmbeddedFontKind::Bold),
        "italic" => Ok(oxoffice_docx::EmbeddedFontKind::Italic),
        "bold_italic" => Ok(oxoffice_docx::EmbeddedFontKind::BoldItalic),
        _ => Err(PyValueError::new_err(
            "font kind must be regular, bold, italic or bold_italic",
        )),
    }
}
pub(super) fn checked_font_license(
    license: &PyFontEmbeddingLicense,
) -> PyResult<oxoffice_docx::FontEmbeddingLicense> {
    if !license.authorized || license.identity.trim().is_empty() {
        return Err(PyValueError::new_err(
            "font embedding requires explicit authorization and nonempty license identity",
        ));
    }
    Ok(oxoffice_docx::FontEmbeddingLicense::new(
        license.authorized,
        license.identity.clone(),
    ))
}

/// The native definition of a descriptive record: `method` takes no faces.
pub(super) fn descriptive_font(
    py: Python<'_>,
    font: &PyFontDefinition,
    method: &str,
) -> PyResult<oxoffice_docx::FontDefinition> {
    if !font.embedded_fonts.bind(py).is_empty() {
        return Err(PyValueError::new_err(format!(
            "{method} accepts descriptive records only, use embed_font"
        )));
    }
    Ok(oxoffice_docx::FontDefinition {
        name: font.name.clone(),
        alternate_name: font.alternate_name.clone(),
        family: font.family.clone(),
        pitch: font.pitch.clone(),
        embedded_fonts: Vec::new(),
    })
}

/// The checked native face `embed_font` and `embed_font_at` take.
pub(super) fn embedded_face(
    py: Python<'_>,
    font: &Bound<'_, PyAny>,
    kind: &str,
    license: &PyFontEmbeddingLicense,
    font_key: Option<String>,
    subsetted: bool,
) -> PyResult<oxoffice_docx::EmbeddedFont> {
    let kind = checked_font_kind(kind)?;
    let license = checked_font_license(license)?;
    let data = checked_font_input(font)?;
    let key = match font_key {
        Some(key) => key,
        None => oxoffice_docx::derive_font_key(&data).map_err(|e| docx_to_pyerr(py, e))?,
    };
    Ok(oxoffice_docx::EmbeddedFont::new(kind, data, key, license).with_subsetted(subsetted))
}

#[pyclass(name = "EmbeddedFont", frozen, get_all, skip_from_py_object)]
pub struct PyEmbeddedFont {
    pub kind: &'static str,
    pub data: Py<PyBytes>,
    pub font_key: String,
    pub subsetted: bool,
    pub license: Py<PyFontEmbeddingLicense>,
}
impl PyEmbeddedFont {
    pub(super) fn snapshot(
        py: Python<'_>,
        font: oxoffice_docx::EmbeddedFont,
    ) -> PyResult<Py<Self>> {
        let data = PyBytes::new_with(py, font.data.len(), |out| {
            out.copy_from_slice(&font.data);
            Ok(())
        })?
        .unbind();
        let license = Py::new(
            py,
            PyFontEmbeddingLicense {
                authorized: font.license.authorized,
                identity: font.license.identity,
            },
        )?;
        Py::new(
            py,
            Self {
                kind: font_kind_name(font.kind),
                data,
                font_key: font.font_key,
                subsetted: font.subsetted,
                license,
            },
        )
    }
}
#[pyclass(name = "FontDefinition", frozen, get_all, skip_from_py_object)]
pub struct PyFontDefinition {
    pub name: String,
    pub alternate_name: Option<String>,
    pub family: Option<String>,
    pub pitch: Option<String>,
    pub embedded_fonts: Py<PyTuple>,
}
#[pymethods]
impl PyFontDefinition {
    #[new]
    #[pyo3(signature = (name, *, alternate_name=None, family=None, pitch=None))]
    fn new(
        py: Python<'_>,
        name: String,
        alternate_name: Option<String>,
        family: Option<String>,
        pitch: Option<String>,
    ) -> PyResult<Self> {
        Ok(Self {
            name,
            alternate_name,
            family,
            pitch,
            embedded_fonts: PyTuple::new(py, Vec::<Py<PyEmbeddedFont>>::new())?.unbind(),
        })
    }
}
impl PyFontDefinition {
    pub(super) fn snapshot(
        py: Python<'_>,
        font: oxoffice_docx::FontDefinition,
    ) -> PyResult<Py<Self>> {
        let mut faces = Vec::new();
        faces
            .try_reserve_exact(font.embedded_fonts.len())
            .map_err(|_| {
                pyo3::exceptions::PyMemoryError::new_err("font snapshot allocation failed")
            })?;
        for font in font.embedded_fonts {
            faces.push(PyEmbeddedFont::snapshot(py, font)?);
        }
        let embedded_fonts = PyTuple::new(py, faces)?.unbind();
        Py::new(
            py,
            Self {
                name: font.name,
                alternate_name: font.alternate_name,
                family: font.family,
                pitch: font.pitch,
                embedded_fonts,
            },
        )
    }
}

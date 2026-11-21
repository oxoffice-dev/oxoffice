//! Embedded fonts, font definitions and embedding licences, and the fonts a caller passes.

use super::*;

pub(super) fn collect_caller_fonts<'py>(
    py: Python<'py>,
    fonts: Option<Vec<(String, Bound<'py, PyBytes>)>>,
    font_dir: Option<PathBuf>,
    font_dirs: Option<Bound<'py, PyAny>>,
) -> PyResult<Option<Vec<oxoffice_layout::FontFile>>> {
    if fonts.is_none() && font_dir.is_none() && font_dirs.is_none() {
        return Ok(None);
    }
    if font_dir.is_some() && font_dirs.is_some() {
        return Err(PyValueError::new_err(
            "font_dir and font_dirs cannot be supplied together",
        ));
    }
    let explicit = fonts
        .unwrap_or_default()
        .into_iter()
        .map(|(family, data)| oxoffice_layout::FontFile {
            family,
            data: data.as_bytes().to_vec(),
        })
        .collect::<Vec<_>>();
    let directories = match font_dirs {
        Some(paths) => {
            if paths.is_instance_of::<PyString>() || paths.is_instance_of::<PyBytes>() {
                return Err(PyTypeError::new_err(
                    "font_dirs must be a sequence of paths, not a scalar string or bytes",
                ));
            }
            paths
                .extract::<Vec<PathBuf>>()
                .map_err(|_| PyTypeError::new_err("font_dirs must be a sequence of paths"))?
        }
        None => font_dir.into_iter().collect(),
    };
    py.detach(|| {
        for directory in &directories {
            let metadata = std::fs::metadata(directory).map_err(|error| {
                std::io::Error::new(
                    error.kind(),
                    format!("font directory {}: {error}", directory.display()),
                )
            })?;
            if !metadata.is_dir() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotADirectory,
                    format!("font directory {} is not a directory", directory.display()),
                ));
            }
        }
        Ok::<_, std::io::Error>(())
    })
    .map_err(PyErr::from)?;
    let selected = py
        .detach(|| {
            let mut selected = Vec::new();
            for directory in directories {
                selected.extend(oxoffice_layout::FontFile::from_dir(&directory)?);
            }
            selected.extend(explicit);
            Ok::<_, oxoffice_layout::LayoutError>(selected)
        })
        .map_err(|error| docx_to_pyerr(py, oxoffice_docx::Error::Layout(error)))?;
    Ok((!selected.is_empty()).then_some(selected))
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

pub(super) fn checked_font_input(font: &Bound<'_, PyAny>) -> PyResult<Vec<u8>> {
    use std::io::Read;
    const LIMIT: usize = 64 * 1024 * 1024;
    let copy = |source: &[u8]| -> PyResult<Vec<u8>> {
        if source.len() > LIMIT {
            return Err(PyValueError::new_err("font exceeds 64 MiB input bound"));
        }
        let mut data = Vec::new();
        data.try_reserve_exact(source.len()).map_err(|_| {
            pyo3::exceptions::PyMemoryError::new_err("font input allocation failed")
        })?;
        data.extend_from_slice(source);
        Ok(data)
    };
    if let Ok(bytes) = font.cast::<PyBytes>() {
        return copy(bytes.as_bytes());
    }
    if let Ok(path) = font.extract::<PathBuf>() {
        let file = std::fs::File::open(path)
            .map_err(|e| pyo3::exceptions::PyOSError::new_err(e.to_string()))?;
        if file
            .metadata()
            .map_err(|e| pyo3::exceptions::PyOSError::new_err(e.to_string()))?
            .len()
            > LIMIT as u64
        {
            return Err(PyValueError::new_err("font exceeds 64 MiB input bound"));
        }
        let mut data = Vec::new();
        let mut input = file.take((LIMIT + 1) as u64);
        let mut chunk = [0u8; 8192];
        loop {
            let count = input
                .read(&mut chunk)
                .map_err(|e| pyo3::exceptions::PyOSError::new_err(e.to_string()))?;
            if count == 0 {
                break;
            }
            if count > LIMIT - data.len() {
                return Err(PyValueError::new_err("font exceeds 64 MiB input bound"));
            }
            data.try_reserve_exact(count).map_err(|_| {
                pyo3::exceptions::PyMemoryError::new_err("font input allocation failed")
            })?;
            data.extend_from_slice(&chunk[..count]);
        }
        return Ok(data);
    }
    let read = font.getattr("read").map_err(|_| {
        PyTypeError::new_err("font must be bytes, a filesystem path or a binary reader")
    })?;
    let mut data = Vec::new();
    loop {
        let remaining = LIMIT - data.len();
        let value = read.call1(((remaining + 1).min(8192),))?;
        let bytes = value
            .cast::<PyBytes>()
            .map_err(|_| PyTypeError::new_err("font reader must return bytes"))?;
        let chunk = bytes.as_bytes();
        if chunk.is_empty() {
            return Ok(data);
        }
        if chunk.len() > remaining {
            return Err(PyValueError::new_err("font exceeds 64 MiB input bound"));
        }
        let needed = data.len() + chunk.len();
        if needed > data.capacity() {
            let capacity = data
                .capacity()
                .saturating_mul(2)
                .clamp(8192, LIMIT)
                .max(needed);
            data.try_reserve_exact(capacity - data.len()).map_err(|_| {
                pyo3::exceptions::PyMemoryError::new_err("font input allocation failed")
            })?;
        }
        data.extend_from_slice(chunk);
    }
}

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

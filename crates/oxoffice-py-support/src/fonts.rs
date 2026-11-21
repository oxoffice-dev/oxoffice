//! The fonts a caller passes to a Python binding: font bytes or files, and font folders.

use std::path::PathBuf;

use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyString};

/// The fonts a caller passes as bytes, a font folder or several folders, read off the GIL. A font that cannot be read
/// raises `layout_error`, so each package keeps its own exception class.
pub fn collect_caller_fonts<'py>(
    py: Python<'py>,
    fonts: Option<Vec<(String, Bound<'py, PyBytes>)>>,
    font_dir: Option<PathBuf>,
    font_dirs: Option<Bound<'py, PyAny>>,
    layout_error: impl FnOnce(oxoffice_layout::LayoutError) -> PyErr,
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
        .map_err(layout_error)?;
    Ok((!selected.is_empty()).then_some(selected))
}

pub fn checked_font_input(font: &Bound<'_, PyAny>) -> PyResult<Vec<u8>> {
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

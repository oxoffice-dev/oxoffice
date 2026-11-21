//! Pictures: their options, the picture input Python gives and the picture handle.

use super::*;

#[pyclass(name = "PictureOptions", frozen, skip_from_py_object)]
#[derive(Clone)]
pub struct PyPictureOptions {
    pub(crate) inner: oxoffice_docx::PictureOptions,
}

#[pymethods]
impl PyPictureOptions {
    #[new]
    #[pyo3(signature = (*, name=None, description=None, title=None, decorative=false, crop=None, placement="inline", horizontal_relative_from="page", vertical_relative_from="page", horizontal_offset=None, vertical_offset=None, horizontal_alignment=None, vertical_alignment=None, distances=None, relative_height=0))]
    #[allow(clippy::too_many_arguments)] // Explicit operands match the native picture vocabulary.
    fn new(
        name: Option<String>,
        description: Option<String>,
        title: Option<String>,
        decorative: bool,
        crop: Option<&Bound<'_, PyAny>>,
        placement: &str,
        horizontal_relative_from: &str,
        vertical_relative_from: &str,
        horizontal_offset: Option<&Bound<'_, PyAny>>,
        vertical_offset: Option<&Bound<'_, PyAny>>,
        horizontal_alignment: Option<&str>,
        vertical_alignment: Option<&str>,
        distances: Option<&Bound<'_, PyAny>>,
        relative_height: u32,
    ) -> PyResult<Self> {
        let horizontal = crate::length_argument(
            horizontal_offset,
            "PictureOptions(horizontal_offset)",
            crate::LengthUnit::Emu,
        )?
        .unwrap_or(0);
        let vertical = crate::length_argument(
            vertical_offset,
            "PictureOptions(vertical_offset)",
            crate::LengthUnit::Emu,
        )?
        .unwrap_or(0);
        let crop = crop
            .map(|crop| {
                let values = crop.cast::<PyTuple>().map_err(|_| {
                    PyTypeError::new_err("picture crop must be a four-number tuple")
                })?;
                if values.len() != 4 {
                    return Err(PyValueError::new_err("picture crop must have four values"));
                }
                let mut out = [0; 4];
                for (index, value) in values.iter().enumerate() {
                    if value.is_instance_of::<PyBool>() {
                        return Err(PyTypeError::new_err(
                            "picture crop values must be numbers, not bool",
                        ));
                    }
                    let value = value.extract::<f64>()?;
                    if !value.is_finite() || !(0.0..1.0).contains(&value) {
                        return Err(PyValueError::new_err(
                            "picture crop values must be finite fractions from zero up to one",
                        ));
                    }
                    out[index] = (value * 100_000.0).trunc() as i32;
                }
                if out[0] + out[2] >= 100_000 || out[1] + out[3] >= 100_000 {
                    return Err(PyValueError::new_err(
                        "picture crop must leave a visible rectangle",
                    ));
                }
                Ok(oxoffice_docx::PictureCrop {
                    left: out[0],
                    top: out[1],
                    right: out[2],
                    bottom: out[3],
                })
            })
            .transpose()?;
        let mut distance = [0; 4];
        if let Some(distances) = distances {
            let values = distances.cast::<PyTuple>().map_err(|_| {
                PyTypeError::new_err("picture distances must be a four-length tuple")
            })?;
            if values.len() != 4 {
                return Err(PyValueError::new_err(
                    "picture distances must have four lengths",
                ));
            }
            for (index, value) in values.iter().enumerate() {
                distance[index] = crate::required_length(
                    &value,
                    "PictureOptions(distances)",
                    crate::LengthUnit::Emu,
                )?;
                if distance[index] < 0 {
                    return Err(PyValueError::new_err(
                        "picture wrap distances cannot be negative",
                    ));
                }
            }
        }
        let h = match horizontal_relative_from {
            "page" => oxoffice_docx::DrawingHorizontalRelativeFrom::Page,
            "margin" => oxoffice_docx::DrawingHorizontalRelativeFrom::Margin,
            "column" => oxoffice_docx::DrawingHorizontalRelativeFrom::Column,
            "character" => oxoffice_docx::DrawingHorizontalRelativeFrom::Character,
            "left_margin" => oxoffice_docx::DrawingHorizontalRelativeFrom::LeftMargin,
            "right_margin" => oxoffice_docx::DrawingHorizontalRelativeFrom::RightMargin,
            "inside_margin" => oxoffice_docx::DrawingHorizontalRelativeFrom::InsideMargin,
            "outside_margin" => oxoffice_docx::DrawingHorizontalRelativeFrom::OutsideMargin,
            _ => {
                return Err(PyValueError::new_err(
                    "unsupported horizontal picture frame",
                ));
            }
        };
        let v = match vertical_relative_from {
            "page" => oxoffice_docx::DrawingVerticalRelativeFrom::Page,
            "margin" => oxoffice_docx::DrawingVerticalRelativeFrom::Margin,
            "paragraph" => oxoffice_docx::DrawingVerticalRelativeFrom::Paragraph,
            "line" => oxoffice_docx::DrawingVerticalRelativeFrom::Line,
            "top_margin" => oxoffice_docx::DrawingVerticalRelativeFrom::TopMargin,
            "bottom_margin" => oxoffice_docx::DrawingVerticalRelativeFrom::BottomMargin,
            "inside_margin" => oxoffice_docx::DrawingVerticalRelativeFrom::InsideMargin,
            "outside_margin" => oxoffice_docx::DrawingVerticalRelativeFrom::OutsideMargin,
            _ => return Err(PyValueError::new_err("unsupported vertical picture frame")),
        };
        let ha = horizontal_alignment
            .map(|value| match value {
                "left" => Ok(oxoffice_docx::DrawingHorizontalAlignment::Left),
                "center" => Ok(oxoffice_docx::DrawingHorizontalAlignment::Center),
                "right" => Ok(oxoffice_docx::DrawingHorizontalAlignment::Right),
                "inside" => Ok(oxoffice_docx::DrawingHorizontalAlignment::Inside),
                "outside" => Ok(oxoffice_docx::DrawingHorizontalAlignment::Outside),
                _ => Err(PyValueError::new_err(
                    "unsupported horizontal picture alignment",
                )),
            })
            .transpose()?;
        let va = vertical_alignment
            .map(|value| match value {
                "top" => Ok(oxoffice_docx::DrawingVerticalAlignment::Top),
                "center" => Ok(oxoffice_docx::DrawingVerticalAlignment::Center),
                "bottom" => Ok(oxoffice_docx::DrawingVerticalAlignment::Bottom),
                "inside" => Ok(oxoffice_docx::DrawingVerticalAlignment::Inside),
                "outside" => Ok(oxoffice_docx::DrawingVerticalAlignment::Outside),
                _ => Err(PyValueError::new_err(
                    "unsupported vertical picture alignment",
                )),
            })
            .transpose()?;
        if (ha.is_some() && horizontal != 0) || (va.is_some() && vertical != 0) {
            return Err(PyValueError::new_err(
                "picture alignment and offset conflict",
            ));
        }
        let anchor = if placement == "inline" {
            if horizontal != 0
                || vertical != 0
                || ha.is_some()
                || va.is_some()
                || distance != [0; 4]
                || relative_height != 0
                || horizontal_relative_from != "page"
                || vertical_relative_from != "page"
            {
                return Err(PyValueError::new_err(
                    "inline pictures cannot have anchor placement operands",
                ));
            }
            None
        } else {
            let wrap = match placement {
                "square" => oxoffice_docx::DrawingWrap::Square,
                "tight" => oxoffice_docx::DrawingWrap::Tight,
                "top_and_bottom" => oxoffice_docx::DrawingWrap::TopAndBottom,
                "behind" | "in_front" => oxoffice_docx::DrawingWrap::None,
                _ => return Err(PyValueError::new_err("unsupported picture placement")),
            };
            Some(oxoffice_docx::PictureAnchor {
                horizontal_relative_from: h,
                horizontal_offset: oxoffice_docx::Length::emu(horizontal),
                horizontal_alignment: ha,
                vertical_relative_from: v,
                vertical_offset: oxoffice_docx::Length::emu(vertical),
                vertical_alignment: va,
                wrap,
                distance_top: oxoffice_docx::Length::emu(distance[0]),
                distance_right: oxoffice_docx::Length::emu(distance[1]),
                distance_bottom: oxoffice_docx::Length::emu(distance[2]),
                distance_left: oxoffice_docx::Length::emu(distance[3]),
                relative_height,
                behind_text: placement == "behind",
            })
        };
        Ok(Self {
            inner: oxoffice_docx::PictureOptions {
                width: oxoffice_docx::Length::emu(1),
                height: oxoffice_docx::Length::emu(1),
                crop,
                anchor,
                name,
                description,
                title,
                decorative,
            },
        })
    }
}

pub(crate) fn picture_input(
    image: &Bound<'_, PyAny>,
    filename: Option<&str>,
) -> PyResult<(Vec<u8>, String)> {
    if let Ok(bytes) = image.cast::<PyBytes>() {
        return Ok((
            bytes.as_bytes().to_vec(),
            filename.unwrap_or("picture").to_owned(),
        ));
    }
    let path = match image.extract::<PathBuf>() {
        Ok(path) => path,
        Err(_) => {
            return Ok((
                binary_stream_bytes(image, false)?,
                filename.unwrap_or("picture").to_owned(),
            ));
        }
    };
    let bytes = std::fs::read(&path)
        .map_err(|error| pyo3::exceptions::PyOSError::new_err(error.to_string()))?;
    Ok((
        bytes,
        filename.map(str::to_owned).unwrap_or_else(|| {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        }),
    ))
}

/// A frozen physical picture, bound to one document and captured revision.
#[pyclass(name = "Picture", frozen, skip_from_py_object)]
pub struct PyPicture {
    pub(crate) inner: oxoffice_docx::PictureOccurrence,
    pub(crate) binding: oxoffice_py_support::ContentPath,
}

#[pymethods]
impl PyPicture {
    #[getter]
    fn width(&self) -> i64 {
        self.inner.width
    }
    #[getter]
    fn height(&self) -> i64 {
        self.inner.height
    }
    #[getter]
    fn relationship_id(&self) -> &str {
        &self.inner.relationship_id
    }
    #[getter]
    fn media_part_name(&self) -> Option<&str> {
        self.inner.media_part_name.as_deref()
    }
    #[getter]
    fn external_url(&self) -> Option<&str> {
        self.inner.external_url.as_deref()
    }
    #[getter]
    fn content_type(&self) -> &str {
        &self.inner.content_type
    }
    #[getter]
    fn description(&self) -> Option<&str> {
        self.inner.description.as_deref()
    }
    #[getter]
    fn title(&self) -> Option<&str> {
        self.inner.title.as_deref()
    }
    #[getter]
    fn name(&self) -> Option<&str> {
        self.inner.name.as_deref()
    }
    #[getter]
    fn decorative(&self) -> bool {
        self.inner.decorative
    }
    #[getter]
    fn anchored(&self) -> bool {
        self.inner.anchored
    }
    #[getter]
    fn bytes<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.inner.bytes)
    }
    #[getter]
    fn story(&self) -> PyStory {
        let mut story = story_snapshot(self.inner.location.story());
        story.binding = Some(self.binding.clone());
        story
    }
    #[getter]
    fn location(&self) -> PyContentLocation {
        let mut story = story_snapshot(self.inner.location.story());
        story.binding = Some(self.binding.clone());
        PyContentLocation {
            story,
            kind: "paragraph".into(),
            index_path: self.inner.location.index_path().to_vec(),
            is_end: false,
        }
    }
    #[getter]
    fn occurrence(&self) -> usize {
        self.inner.ordinal
    }
}

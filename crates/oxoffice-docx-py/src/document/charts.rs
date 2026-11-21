//! Charts: the chart kinds Python names, their checked inputs and the chart snapshot.

use super::*;

const CHART_TYPES: [(i64, oxoffice_docx::ChartKind); 7] = [
    (1, oxoffice_docx::ChartKind::Area),
    (51, oxoffice_docx::ChartKind::Bar),
    (-4120, oxoffice_docx::ChartKind::Doughnut),
    (65, oxoffice_docx::ChartKind::Line),
    (5, oxoffice_docx::ChartKind::Pie),
    (81, oxoffice_docx::ChartKind::Radar),
    (74, oxoffice_docx::ChartKind::Scatter),
];

pub(super) fn checked_chart_inputs(
    chart_type: &Bound<'_, PyAny>,
    data: &Bound<'_, PyAny>,
) -> PyResult<(oxoffice_docx::ChartKind, oxoffice_docx::ChartData)> {
    if chart_type.is_instance_of::<pyo3::types::PyBool>() {
        return Err(PyTypeError::new_err(
            "chart type must be an XL_CHART_TYPE integer",
        ));
    }
    let value = chart_type.extract::<i64>()?;
    let kind = CHART_TYPES
        .iter()
        .find_map(|(v, k)| (*v == value).then_some(*k))
        .ok_or_else(|| PyValueError::new_err(format!("unsupported XL_CHART_TYPE value {value}")))?;
    let (categories, series, number_format) =
        data.call_method0("_data")?
            .extract::<(Vec<String>, Vec<(String, Vec<f64>)>, Option<String>)>()?;
    Ok((
        kind,
        oxoffice_docx::ChartData {
            categories,
            series,
            number_format,
            ..Default::default()
        },
    ))
}

#[pymethods]
impl PyChart {
    #[getter]
    fn chart_type(&self) -> Option<i64> {
        self.chart_type
    }
    #[getter]
    fn categories<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        PyTuple::new(py, &self.data.categories)
    }
    #[getter]
    fn series<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let class = py.import("oxoffice.docx")?.getattr("CategorySeriesData")?;
        let values = self
            .data
            .series
            .iter()
            .map(|(name, values)| {
                class.call1((name, values.clone(), self.data.number_format.clone()))
            })
            .collect::<PyResult<Vec<_>>>()?;
        PyTuple::new(py, values)
    }
}

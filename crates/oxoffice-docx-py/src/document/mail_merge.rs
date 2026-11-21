//! Mail merge settings, recipients, field mappings, package sources and their options.

use super::*;

fn empty_py_tuple() -> Py<PyAny> {
    Python::attach(|py| PyTuple::empty(py).into_any().unbind())
}

fn true_py_bool() -> Py<PyAny> {
    Python::attach(|py| PyBool::new(py, true).to_owned().into_any().unbind())
}

fn exact_str<'a>(value: &'a Bound<'a, PyAny>, label: &str) -> PyResult<std::borrow::Cow<'a, str>> {
    if !value.is_exact_instance_of::<PyString>() {
        return Err(PyTypeError::new_err(format!("{label} must be str")));
    }
    value
        .cast::<PyString>()
        .map_err(|_| PyTypeError::new_err(format!("{label} must be str")))?
        .to_cow()
        .map_err(|_| PyTypeError::new_err(format!("{label} must be str")))
}

fn extract_strict_bool(value: &Bound<'_, PyAny>, label: &str) -> PyResult<bool> {
    if !value.is_exact_instance_of::<PyBool>() {
        return Err(PyTypeError::new_err(format!("{label} must be bool")));
    }
    value.extract()
}

fn extract_optional_bool(value: Option<&Bound<'_, PyAny>>, label: &str) -> PyResult<Option<bool>> {
    match value {
        None => Ok(None),
        Some(value) if value.is_none() => Ok(None),
        Some(value) => Ok(Some(extract_strict_bool(value, label)?)),
    }
}

fn extract_optional_text(
    value: Option<&Bound<'_, PyAny>>,
    label: &str,
) -> PyResult<Option<String>> {
    match value {
        None => Ok(None),
        Some(value) if value.is_none() => Ok(None),
        Some(value) => Ok(Some(exact_str(value, label)?.into_owned())),
    }
}

fn extract_u32_value(value: &Bound<'_, PyAny>, label: &str) -> PyResult<u32> {
    if value.is_instance_of::<PyBool>() || !value.is_instance_of::<PyInt>() {
        return Err(PyTypeError::new_err(format!("{label} must be an int")));
    }
    if value.lt(0).unwrap_or(false) {
        return Err(PyValueError::new_err(format!(
            "{label} must be zero or positive"
        )));
    }
    value.extract::<u32>()
}

fn extract_optional_i32(value: Option<&Bound<'_, PyAny>>, label: &str) -> PyResult<Option<i32>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_none() {
        return Ok(None);
    }
    if value.is_instance_of::<PyBool>() || !value.is_instance_of::<PyInt>() {
        return Err(PyTypeError::new_err(format!("{label} must be an int")));
    }
    let negative = value.lt(0).unwrap_or(false);
    match value.extract::<i32>() {
        Ok(number) => Ok(Some(number)),
        Err(_) if negative => Err(PyValueError::new_err(format!(
            "{label} is below the i32 range"
        ))),
        Err(error) => Err(error),
    }
}

fn bounded_sequence_len(value: &Bound<'_, PyAny>, cap: usize, label: &str) -> PyResult<usize> {
    let reported = value.len()?;
    if reported > cap {
        return Err(PyValueError::new_err(format!(
            "{label} exceeds the mail-merge table limit"
        )));
    }
    Ok(reported)
}

fn parse_main_document_type(
    value: Option<&str>,
) -> PyResult<Option<oxoffice_docx::MailMergeDocumentType>> {
    let Some(value) = value else {
        return Ok(None);
    };
    Ok(Some(match value {
        "catalog" => oxoffice_docx::MailMergeDocumentType::Catalog,
        "envelopes" => oxoffice_docx::MailMergeDocumentType::Envelopes,
        "mailingLabels" => oxoffice_docx::MailMergeDocumentType::MailingLabels,
        "formLetters" => oxoffice_docx::MailMergeDocumentType::FormLetters,
        "email" => oxoffice_docx::MailMergeDocumentType::Email,
        "fax" => oxoffice_docx::MailMergeDocumentType::Fax,
        _ => {
            return Err(PyValueError::new_err(
                "main_document_type is not a mail-merge document type",
            ));
        }
    }))
}

fn main_document_type_name(value: oxoffice_docx::MailMergeDocumentType) -> &'static str {
    match value {
        oxoffice_docx::MailMergeDocumentType::Catalog => "catalog",
        oxoffice_docx::MailMergeDocumentType::Envelopes => "envelopes",
        oxoffice_docx::MailMergeDocumentType::MailingLabels => "mailingLabels",
        oxoffice_docx::MailMergeDocumentType::FormLetters => "formLetters",
        oxoffice_docx::MailMergeDocumentType::Email => "email",
        oxoffice_docx::MailMergeDocumentType::Fax => "fax",
    }
}

fn parse_destination(value: Option<&str>) -> PyResult<Option<oxoffice_docx::MailMergeDestination>> {
    let Some(value) = value else {
        return Ok(None);
    };
    Ok(Some(match value {
        "newDocument" => oxoffice_docx::MailMergeDestination::NewDocument,
        "printer" => oxoffice_docx::MailMergeDestination::Printer,
        "email" => oxoffice_docx::MailMergeDestination::Email,
        "fax" => oxoffice_docx::MailMergeDestination::Fax,
        _ => {
            return Err(PyValueError::new_err(
                "destination is not a mail-merge destination",
            ));
        }
    }))
}

fn destination_name(value: oxoffice_docx::MailMergeDestination) -> &'static str {
    match value {
        oxoffice_docx::MailMergeDestination::NewDocument => "newDocument",
        oxoffice_docx::MailMergeDestination::Printer => "printer",
        oxoffice_docx::MailMergeDestination::Email => "email",
        oxoffice_docx::MailMergeDestination::Fax => "fax",
    }
}

fn default_mail_merge_settings() -> oxoffice_docx::MailMerge {
    oxoffice_docx::MailMerge {
        main_document_type: Some(oxoffice_docx::MailMergeDocumentType::FormLetters),
        data_type: Some("textFile".to_owned()),
        ..oxoffice_docx::MailMerge::default()
    }
}

fn embedded_package_source(
    py: Python<'_>,
    columns: &Bound<'_, PyAny>,
    records: &Bound<'_, PyAny>,
) -> PyResult<oxoffice_docx::MailMergePackageSource> {
    require_carrier_sequence(columns, "columns")?;
    require_carrier_sequence(records, "records")?;
    let column_len = bounded_sequence_len(columns, 1024, "columns")?;
    let row_len = bounded_sequence_len(records, 100_000, "records")?;
    let mut budget = oxoffice_docx::MailMergeTableBudget::new();
    let mut owned_columns = Vec::with_capacity(column_len);
    for item in columns.try_iter()? {
        let item = item?;
        let name = exact_str(&item, "column")?;
        budget
            .add_column(&name)
            .map_err(|error| docx_to_pyerr(py, error))?;
        owned_columns.push(name.into_owned());
    }
    let mut owned_records = Vec::with_capacity(row_len);
    for row in records.try_iter()? {
        let row = row?;
        require_carrier_sequence(&row, "record")?;
        let cell_len = row.len()?.min(owned_columns.len());
        budget
            .begin_record()
            .map_err(|error| docx_to_pyerr(py, error))?;
        let mut owned_row = Vec::with_capacity(cell_len);
        for cell in row.try_iter()? {
            let cell = cell?;
            let text = exact_str(&cell, "cell")?;
            budget
                .add_cell(&text)
                .map_err(|error| docx_to_pyerr(py, error))?;
            owned_row.push(text.into_owned());
        }
        budget
            .end_record()
            .map_err(|error| docx_to_pyerr(py, error))?;
        owned_records.push(owned_row);
    }
    budget.finish().map_err(|error| docx_to_pyerr(py, error))?;
    Ok(oxoffice_docx::MailMergePackageSource::Embedded {
        columns: owned_columns,
        records: owned_records,
    })
}

fn copied_mappings(
    value: &Bound<'_, PyAny>,
) -> PyResult<Vec<oxoffice_docx::MailMergeFieldMapping>> {
    require_carrier_sequence(value, "mappings")?;
    let mut mappings = Vec::new();
    for item in value.try_iter()? {
        let item = item?;
        let mapping = item.cast::<PyMailMergeFieldMapping>().map_err(|_| {
            PyTypeError::new_err("mappings must be a sequence of MailMergeFieldMapping")
        })?;
        mappings.push(mapping.borrow().inner.clone());
    }
    Ok(mappings)
}

fn copied_recipients(value: &Bound<'_, PyAny>) -> PyResult<Vec<oxoffice_docx::MailMergeRecipient>> {
    require_carrier_sequence(value, "recipients")?;
    let mut recipients = Vec::new();
    for item in value.try_iter()? {
        let item = item?;
        let recipient = item.cast::<PyMailMergeRecipient>().map_err(|_| {
            PyTypeError::new_err("recipients must be a sequence of MailMergeRecipient")
        })?;
        recipients.push(recipient.borrow().inner.clone());
    }
    Ok(recipients)
}

pub(super) fn copied_external_rows(
    py: Python<'_>,
    value: &Bound<'_, PyAny>,
) -> PyResult<Vec<BTreeMap<String, String>>> {
    let mapping = py.import("collections.abc")?.getattr("Mapping")?;
    let mut rows = Vec::new();
    for item in value.try_iter()? {
        let item = item?;
        if item.is_instance_of::<PyString>() || !item.is_instance(&mapping)? {
            return Err(PyTypeError::new_err(
                "external record must be a mapping of str to str",
            ));
        }
        let mut row = BTreeMap::new();
        for pair in item.call_method0("items")?.try_iter()? {
            let pair = pair?;
            let pair = pair
                .cast::<PyTuple>()
                .map_err(|_| PyTypeError::new_err("external record items must be pairs"))?;
            let key_item = pair.get_item(0)?;
            let value_item = pair.get_item(1)?;
            let key = exact_str(&key_item, "external record key")?;
            let cell = exact_str(&value_item, "external record value")?;
            row.insert(key.into_owned(), cell.into_owned());
        }
        rows.push(row);
    }
    Ok(rows)
}

fn string_tuple<'py>(py: Python<'py>, values: &[String]) -> PyResult<Bound<'py, PyTuple>> {
    PyTuple::new(py, values.iter().map(String::as_str))
}

#[pyclass(frozen, name = "MailMergePackageSource")]
pub(crate) struct PyMailMergePackageSource {
    inner: oxoffice_docx::MailMergePackageSource,
}

#[pymethods]
impl PyMailMergePackageSource {
    #[staticmethod]
    #[pyo3(signature = (columns, records), text_signature = "(columns, records)")]
    fn embedded(
        py: Python<'_>,
        columns: &Bound<'_, PyAny>,
        records: &Bound<'_, PyAny>,
    ) -> PyResult<Self> {
        Ok(Self {
            inner: embedded_package_source(py, columns, records)?,
        })
    }

    #[staticmethod]
    #[pyo3(signature = (uri,), text_signature = "(uri)")]
    fn external(py: Python<'_>, uri: &Bound<'_, PyAny>) -> PyResult<Self> {
        let uri = exact_str(uri, "uri")?;
        oxoffice_docx::validate_mail_merge_external_uri(&uri)
            .map_err(|error| docx_to_pyerr(py, error))?;
        Ok(Self {
            inner: oxoffice_docx::MailMergePackageSource::External {
                uri: uri.into_owned(),
            },
        })
    }

    #[getter]
    fn kind(&self) -> &'static str {
        match self.inner {
            oxoffice_docx::MailMergePackageSource::Embedded { .. } => "embedded",
            oxoffice_docx::MailMergePackageSource::External { .. } => "external",
        }
    }

    #[getter]
    fn columns<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        match &self.inner {
            oxoffice_docx::MailMergePackageSource::Embedded { columns, .. } => {
                string_tuple(py, columns)
            }
            oxoffice_docx::MailMergePackageSource::External { .. } => Ok(PyTuple::empty(py)),
        }
    }

    #[getter]
    fn records<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        match &self.inner {
            oxoffice_docx::MailMergePackageSource::Embedded { records, .. } => {
                let mut rows = Vec::with_capacity(records.len());
                for record in records {
                    rows.push(string_tuple(py, record)?.into_any());
                }
                PyTuple::new(py, rows)
            }
            oxoffice_docx::MailMergePackageSource::External { .. } => Ok(PyTuple::empty(py)),
        }
    }

    #[getter]
    fn uri(&self) -> Option<&str> {
        match &self.inner {
            oxoffice_docx::MailMergePackageSource::External { uri } => Some(uri),
            oxoffice_docx::MailMergePackageSource::Embedded { .. } => None,
        }
    }
}

#[pyclass(frozen, name = "MailMergeSettings")]
pub(crate) struct PyMailMergeSettings {
    inner: oxoffice_docx::MailMerge,
}

#[pymethods]
impl PyMailMergeSettings {
    #[new]
    #[pyo3(
        signature = (
            *,
            main_document_type = Some("formLetters"),
            link_to_query = None,
            data_type = Some("textFile"),
            connect_string = None,
            query = None,
            do_not_suppress_blank_lines = None,
            destination = None,
            address_field_name = None,
            mail_subject = None,
            mail_as_attachment = None,
            view_merged_data = None,
            active_record = None,
            check_errors = None,
        ),
        text_signature = "(*, main_document_type='formLetters', link_to_query=None, data_type='textFile', connect_string=None, query=None, do_not_suppress_blank_lines=None, destination=None, address_field_name=None, mail_subject=None, mail_as_attachment=None, view_merged_data=None, active_record=None, check_errors=None)"
    )]
    #[allow(clippy::too_many_arguments)] // The approved settings carrier keeps thirteen explicit operands.
    fn new(
        main_document_type: Option<&str>,
        link_to_query: Option<&Bound<'_, PyAny>>,
        data_type: Option<&str>,
        connect_string: Option<&str>,
        query: Option<&str>,
        do_not_suppress_blank_lines: Option<&Bound<'_, PyAny>>,
        destination: Option<&str>,
        address_field_name: Option<&str>,
        mail_subject: Option<&str>,
        mail_as_attachment: Option<&Bound<'_, PyAny>>,
        view_merged_data: Option<&Bound<'_, PyAny>>,
        active_record: Option<&Bound<'_, PyAny>>,
        check_errors: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        Ok(Self {
            inner: oxoffice_docx::MailMerge {
                main_document_type: parse_main_document_type(main_document_type)?,
                link_to_query: extract_optional_bool(link_to_query, "link_to_query")?,
                data_type: data_type.map(str::to_owned),
                connect_string: connect_string.map(str::to_owned),
                query: query.map(str::to_owned),
                do_not_suppress_blank_lines: extract_optional_bool(
                    do_not_suppress_blank_lines,
                    "do_not_suppress_blank_lines",
                )?,
                destination: parse_destination(destination)?,
                address_field_name: address_field_name.map(str::to_owned),
                mail_subject: mail_subject.map(str::to_owned),
                mail_as_attachment: extract_optional_bool(
                    mail_as_attachment,
                    "mail_as_attachment",
                )?,
                view_merged_data: extract_optional_bool(view_merged_data, "view_merged_data")?,
                active_record: extract_optional_i32(active_record, "active_record")?,
                check_errors: extract_optional_i32(check_errors, "check_errors")?,
            },
        })
    }

    #[getter]
    fn main_document_type(&self) -> Option<&'static str> {
        self.inner.main_document_type.map(main_document_type_name)
    }
    #[getter]
    fn link_to_query(&self) -> Option<bool> {
        self.inner.link_to_query
    }
    #[getter]
    fn data_type(&self) -> Option<&str> {
        self.inner.data_type.as_deref()
    }
    #[getter]
    fn connect_string(&self) -> Option<&str> {
        self.inner.connect_string.as_deref()
    }
    #[getter]
    fn query(&self) -> Option<&str> {
        self.inner.query.as_deref()
    }
    #[getter]
    fn do_not_suppress_blank_lines(&self) -> Option<bool> {
        self.inner.do_not_suppress_blank_lines
    }
    #[getter]
    fn destination(&self) -> Option<&'static str> {
        self.inner.destination.map(destination_name)
    }
    #[getter]
    fn address_field_name(&self) -> Option<&str> {
        self.inner.address_field_name.as_deref()
    }
    #[getter]
    fn mail_subject(&self) -> Option<&str> {
        self.inner.mail_subject.as_deref()
    }
    #[getter]
    fn mail_as_attachment(&self) -> Option<bool> {
        self.inner.mail_as_attachment
    }
    #[getter]
    fn view_merged_data(&self) -> Option<bool> {
        self.inner.view_merged_data
    }
    #[getter]
    fn active_record(&self) -> Option<i32> {
        self.inner.active_record
    }
    #[getter]
    fn check_errors(&self) -> Option<i32> {
        self.inner.check_errors
    }
}

#[pyclass(frozen, name = "MailMergeFieldMapping")]
pub(crate) struct PyMailMergeFieldMapping {
    inner: oxoffice_docx::MailMergeFieldMapping,
}

#[pymethods]
impl PyMailMergeFieldMapping {
    #[new]
    #[pyo3(
        signature = (column, source_name, mapped_name, *, language = None, dynamic_address = None),
        text_signature = "(column, source_name, mapped_name, *, language=None, dynamic_address=None)"
    )]
    fn new(
        column: &Bound<'_, PyAny>,
        source_name: &Bound<'_, PyAny>,
        mapped_name: &Bound<'_, PyAny>,
        language: Option<&Bound<'_, PyAny>>,
        dynamic_address: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        Ok(Self {
            inner: oxoffice_docx::MailMergeFieldMapping {
                column: extract_u32_value(column, "column")?,
                source_name: exact_str(source_name, "source_name")?.into_owned(),
                mapped_name: exact_str(mapped_name, "mapped_name")?.into_owned(),
                language: extract_optional_text(language, "language")?,
                dynamic_address: extract_optional_bool(dynamic_address, "dynamic_address")?,
            },
        })
    }

    #[getter]
    fn column(&self) -> u32 {
        self.inner.column
    }
    #[getter]
    fn source_name(&self) -> &str {
        &self.inner.source_name
    }
    #[getter]
    fn mapped_name(&self) -> &str {
        &self.inner.mapped_name
    }
    #[getter]
    fn language(&self) -> Option<&str> {
        self.inner.language.as_deref()
    }
    #[getter]
    fn dynamic_address(&self) -> Option<bool> {
        self.inner.dynamic_address
    }
}

#[pyclass(frozen, name = "MailMergeRecipient")]
pub(crate) struct PyMailMergeRecipient {
    inner: oxoffice_docx::MailMergeRecipient,
}

#[pymethods]
impl PyMailMergeRecipient {
    #[new]
    #[pyo3(
        signature = (column, unique_tag, *, active = true_py_bool()),
        text_signature = "(column, unique_tag, *, active=True)"
    )]
    fn new(
        column: &Bound<'_, PyAny>,
        unique_tag: &Bound<'_, PyAny>,
        active: Py<PyAny>,
    ) -> PyResult<Self> {
        let py = unique_tag.py();
        if !unique_tag.is_exact_instance_of::<PyBytes>() {
            return Err(PyTypeError::new_err("unique_tag must be bytes"));
        }
        Ok(Self {
            inner: oxoffice_docx::MailMergeRecipient {
                column: extract_u32_value(column, "column")?,
                unique_tag: unique_tag
                    .cast::<PyBytes>()
                    .map_err(|_| PyTypeError::new_err("unique_tag must be bytes"))?
                    .as_bytes()
                    .to_vec(),
                active: extract_strict_bool(active.bind(py), "active")?,
            },
        })
    }

    #[getter]
    fn column(&self) -> u32 {
        self.inner.column
    }
    #[getter]
    fn unique_tag<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.inner.unique_tag)
    }
    #[getter]
    fn active(&self) -> bool {
        self.inner.active
    }
}

#[pymethods]
impl PyMailMergePackageOptions {
    #[new]
    #[pyo3(
        signature = (source, *, settings = None, mappings = empty_py_tuple(), recipients = empty_py_tuple()),
        text_signature = "(source, *, settings=None, mappings=(), recipients=())"
    )]
    fn new(
        source: &Bound<'_, PyMailMergePackageSource>,
        settings: Option<&Bound<'_, PyMailMergeSettings>>,
        mappings: Py<PyAny>,
        recipients: Py<PyAny>,
    ) -> PyResult<Self> {
        let py = source.py();
        Ok(Self {
            inner: oxoffice_docx::MailMergePackageOptions {
                source: source.borrow().inner.clone(),
                settings: settings
                    .map(|settings| settings.borrow().inner.clone())
                    .unwrap_or_else(default_mail_merge_settings),
                mappings: copied_mappings(mappings.bind(py))?,
                recipients: copied_recipients(recipients.bind(py))?,
            },
        })
    }

    #[getter]
    fn source(&self) -> PyMailMergePackageSource {
        PyMailMergePackageSource {
            inner: self.inner.source.clone(),
        }
    }

    #[getter]
    fn settings(&self) -> PyMailMergeSettings {
        PyMailMergeSettings {
            inner: self.inner.settings.clone(),
        }
    }

    #[getter]
    fn mappings<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let mut items = Vec::with_capacity(self.inner.mappings.len());
        for mapping in &self.inner.mappings {
            items.push(
                PyMailMergeFieldMapping {
                    inner: mapping.clone(),
                }
                .into_pyobject(py)?,
            );
        }
        PyTuple::new(py, items)
    }

    #[getter]
    fn recipients<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let mut items = Vec::with_capacity(self.inner.recipients.len());
        for recipient in &self.inner.recipients {
            items.push(
                PyMailMergeRecipient {
                    inner: recipient.clone(),
                }
                .into_pyobject(py)?,
            );
        }
        PyTuple::new(py, items)
    }
}

#[pyclass(frozen, name = "MailMergePackageDiagnostic")]
pub(crate) struct PyMailMergePackageDiagnostic {
    inner: oxoffice_docx::MailMergePackageDiagnostic,
}

#[pymethods]
impl PyMailMergePackageDiagnostic {
    #[getter]
    fn kind(&self) -> &'static str {
        match self.inner.kind {
            oxoffice_docx::MailMergePackageDiagnosticKind::ExternalSourceBlocked => {
                "external_source_blocked"
            }
            oxoffice_docx::MailMergePackageDiagnosticKind::ExternalSourceUnavailable => {
                "external_source_unavailable"
            }
            oxoffice_docx::MailMergePackageDiagnosticKind::UnsupportedSource => {
                "unsupported_source"
            }
        }
    }

    #[getter]
    fn message(&self) -> &str {
        &self.inner.message
    }
}

#[pymethods]
impl PyMailMergePreparedData {
    #[getter]
    fn records<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let mut items = Vec::with_capacity(self.inner.records.len());
        for record in &self.inner.records {
            let dict = PyDict::new(py);
            for (key, value) in record {
                dict.set_item(key, value)?;
            }
            items.push(dict.into_any());
        }
        PyTuple::new(py, items)
    }

    #[getter]
    fn diagnostics<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyTuple>> {
        let mut items = Vec::with_capacity(self.inner.diagnostics.len());
        for diagnostic in &self.inner.diagnostics {
            items.push(
                PyMailMergePackageDiagnostic {
                    inner: diagnostic.clone(),
                }
                .into_pyobject(py)?,
            );
        }
        PyTuple::new(py, items)
    }
}

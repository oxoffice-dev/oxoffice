//! Shared path, revision, and unit support for Python OOXML bindings.

pub mod core_properties;

use smallvec::SmallVec;
use thiserror::Error;

use oxoffice_core::Length;
use pyo3::exceptions::{PyOverflowError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyString;

/// Validate an RGBColor, a legacy channel triple, or optional-hash ASCII hex.
/// Callers handle their own None and auto policies before extraction.
pub fn rgb_channels(value: &Bound<'_, PyAny>, parameter: &str) -> PyResult<(u8, u8, u8)> {
    if value.is_instance_of::<PyString>() {
        let malformed = || {
            PyValueError::new_err(format!(
                "{parameter} must contain exactly six ASCII hexadecimal digits, optionally preceded by one #"
            ))
        };
        let source = value.extract::<String>().map_err(|_| malformed())?;
        let hex = source.strip_prefix('#').unwrap_or(&source);
        if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(malformed());
        }
        let channel = |start| {
            u8::from_str_radix(&hex[start..start + 2], 16)
                .expect("six ASCII hexadecimal digits were validated")
        };
        return Ok((channel(0), channel(2), channel(4)));
    }
    let (red, green, blue) = value.extract::<(i64, i64, i64)>().map_err(|error| {
        if error.is_instance_of::<PyOverflowError>(value.py()) {
            return PyValueError::new_err(format!("{parameter} channels must be integers from 0 to 255"));
        }
        PyTypeError::new_err(format!(
            "{parameter} accepts RGBColor, a legacy triple of integer channels, or six ASCII hexadecimal digits with optional #, got {}",
            value.get_type().name().map(|name| name.to_string()).unwrap_or_else(|_| "unknown type".to_owned())
        ))
    })?;
    let channel = |channel| {
        u8::try_from(channel).map_err(|_| {
            PyValueError::new_err(format!(
                "{parameter} channels must be integers from 0 to 255"
            ))
        })
    };
    Ok((channel(red)?, channel(green)?, channel(blue)?))
}

/// One index step in a Word or PowerPoint content handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PathSeg {
    /// A slide in a presentation.
    Slide(usize),
    /// A layout in a presentation.
    Layout(usize),
    /// A master in a presentation.
    Master(usize),
    /// A title, body or other master text style selected by its fixed slot.
    TextStyle(usize),
    /// A shape in a slide or group shape.
    Shape(usize),
    /// An item in the document body's block content.
    Body(usize),
    /// A row in a table.
    Row(usize),
    /// A cell in a table row.
    Cell(usize),
    /// A paragraph in its parent container.
    Para(usize),
    /// A run in a paragraph.
    Run(usize),
}

/// An index path paired with the document revision at which it was captured.
#[derive(Debug, Clone)]
pub struct ContentPath {
    /// Ordered index steps from the document root to the content.
    pub segs: SmallVec<[PathSeg; 5]>,
    /// Document revision at handle creation.
    pub revision: u64,
    /// Optional cell-text generation inherited by nested presentation handles.
    pub text_revision: Option<u64>,
    binding: Option<PathBinding>,
}

impl ContentPath {
    /// Capture a path at `revision`.
    pub const fn new(segs: SmallVec<[PathSeg; 5]>, revision: u64) -> Self {
        Self {
            segs,
            revision,
            text_revision: None,
            binding: None,
        }
    }

    /// Reject a handle captured before or after `current_revision`.
    pub fn validate_revision(
        &self,
        current_revision: u64,
        element_kind: &str,
        recovery_hint: &str,
    ) -> Result<(), StaleElementError> {
        if self.revision == current_revision {
            return Ok(());
        }

        Err(StaleElementError {
            element_kind: element_kind.to_owned(),
            captured_revision: self.revision,
            current_revision,
            recovery_hint: recovery_hint.to_owned(),
        })
    }
}

/// A content handle whose captured revision no longer matches its document.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error(
    "stale {element_kind} handle was created at document revision {captured_revision}, but the document is now at revision {current_revision} (a structural change invalidated it). {recovery_hint}"
)]
pub struct StaleElementError {
    /// Human-readable handle kind, such as `paragraph` or `run`.
    pub element_kind: String,
    /// Revision captured when the handle was created.
    pub captured_revision: u64,
    /// Revision owned by the document when the handle was used.
    pub current_revision: u64,
    /// Caller-supplied guidance for obtaining a fresh handle.
    pub recovery_hint: String,
}

/// Relevant invalidation partitions shared by the two binding families.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RevisionScope {
    Document,
    Owner,
    Grid,
    Text,
    SlideInventory,
    LayoutInventory,
    MasterInventory,
}

#[derive(Debug)]
struct ScopeGeneration {
    origin: std::sync::Arc<()>,
    generation: u64,
    scope: RevisionScope,
    retired: std::sync::OnceLock<InvalidationCause>,
}

/// Hashes an allocation address, unique while the allocation is held, for
/// the scope set of one preparation.
#[derive(Default)]
struct AddressHasher(u64);

impl std::hash::Hasher for AddressHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = (self.0.rotate_left(8) ^ u64::from(*byte)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        }
    }

    fn write_usize(&mut self, address: usize) {
        self.0 = (address as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    }
}

/// One immutable generation retained by the current inventory and live handles.
#[derive(Debug, Clone)]
pub struct RevisionState(std::sync::Arc<ScopeGeneration>);

/// The first successful public mutation that retired a generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidationCause {
    pub serial: u64,
    pub operation: Option<&'static str>,
    pub scope: RevisionScope,
}

impl RevisionState {
    pub fn generation(&self) -> u64 {
        self.0.generation
    }
    pub fn scope(&self) -> RevisionScope {
        self.0.scope
    }
    pub fn first_cause(&self) -> Option<&InvalidationCause> {
        self.0.retired.get()
    }
    pub fn same_state(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Debug, Clone)]
struct PathBinding {
    origin: std::sync::Arc<()>,
    states: SmallVec<[RevisionState; 4]>,
}

/// Prepublication validation failure, with no counter or token mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RevisionError {
    #[error("document transaction serial or owner generation overflowed")]
    Overflow,
    #[error("scope belongs to a different document")]
    ForeignOwner,
    #[error("scope generation is already retired")]
    Retired,
    #[error("scope occurs more than once in one transaction")]
    DuplicateScope,
    #[error("prepared transaction no longer matches the owning counter")]
    StalePreparation,
    #[error("initial owner inventory was already initialized")]
    AlreadyInitialized,
    #[error("unable to allocate the prepared owner inventory")]
    Allocation,
}

/// Preserve checked overflow and allocation failure before native publication.
pub fn revision_to_pyerr(error: RevisionError) -> PyErr {
    match error {
        RevisionError::Overflow => PyOverflowError::new_err(error.to_string()),
        RevisionError::Allocation => pyo3::exceptions::PyMemoryError::new_err(error.to_string()),
        _ => PyValueError::new_err(error.to_string()),
    }
}

/// Fully allocated requested successors and births. Dropping publishes nothing.
#[derive(Debug)]
pub struct PreparedRevision {
    origin: std::sync::Arc<()>,
    expected_serial: u64,
    expected_generation: u64,
    next_generation: u64,
    operation: Option<&'static str>,
    retire_document: bool,
    retired: Vec<RevisionState>,
    successors: Vec<RevisionState>,
    born: Vec<RevisionState>,
}

impl PreparedRevision {
    /// Requested successors follow the caller's affected scopes. Replacement has none.
    pub fn successor(&self, index: usize) -> Option<RevisionState> {
        self.successors.get(index.checked_add(1)?).cloned()
    }
    pub fn committed_serial(&self) -> u64 {
        self.expected_serial + 1
    }

    /// The actual document state that this preparation will publish.
    pub fn document_scope(&self) -> RevisionState {
        self.successors[0].clone()
    }
    pub fn born(&self, index: usize) -> Option<RevisionState> {
        self.born.get(index).cloned()
    }
}

/// The three actual publication policies for prepared document and owner states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionPreparation {
    /// Retire the document and supply every requested owner successor.
    Global,
    /// Keep the document epoch and supply every requested owner successor.
    Local,
    /// Retire the document and supply births, reserving unused owner generations.
    Replacement,
}

/// One document's common committed serial and checked generation allocator.
#[derive(Debug)]
pub struct RevisionCounter {
    current: u64,
    next_generation: u64,
    origin: std::sync::Arc<()>,
    document: RevisionState,
    initialized: bool,
}

impl Default for RevisionCounter {
    fn default() -> Self {
        Self::new()
    }
}
impl PartialEq for RevisionCounter {
    fn eq(&self, other: &Self) -> bool {
        self.current == other.current
    }
}
impl Eq for RevisionCounter {}

impl RevisionCounter {
    pub fn new() -> Self {
        let origin = std::sync::Arc::new(());
        let document = RevisionState(std::sync::Arc::new(ScopeGeneration {
            origin: origin.clone(),
            generation: 0,
            scope: RevisionScope::Document,
            retired: std::sync::OnceLock::new(),
        }));
        Self {
            current: 0,
            next_generation: 0,
            origin,
            document,
            initialized: false,
        }
    }
    pub const fn current(&self) -> u64 {
        self.current
    }

    /// Capture the real document epoch after eager owner initialization.
    /// Global commits retire this state, while local owner commits retain it.
    pub fn current_document_scope(&self) -> RevisionState {
        self.document.clone()
    }

    fn owns(&self, state: &RevisionState) -> bool {
        std::sync::Arc::ptr_eq(&self.origin, &state.0.origin)
    }

    fn state(&self, generation: u64, scope: RevisionScope) -> RevisionState {
        RevisionState(std::sync::Arc::new(ScopeGeneration {
            origin: self.origin.clone(),
            generation,
            scope,
            retired: std::sync::OnceLock::new(),
        }))
    }

    /// Only construction uses this operation, before returning captured handles.
    pub fn initialize_scopes(
        &mut self,
        scopes: &[RevisionScope],
    ) -> Result<Vec<RevisionState>, RevisionError> {
        if self.initialized || std::sync::Arc::strong_count(&self.document.0) != 1 {
            return Err(RevisionError::AlreadyInitialized);
        }
        let count = u64::try_from(scopes.len()).map_err(|_| RevisionError::Overflow)?;
        let last = self
            .next_generation
            .checked_add(count)
            .ok_or(RevisionError::Overflow)?;
        let mut result = Vec::new();
        result
            .try_reserve_exact(scopes.len())
            .map_err(|_| RevisionError::Allocation)?;
        for (index, scope) in scopes.iter().enumerate() {
            result.push(self.state(self.next_generation + index as u64 + 1, *scope));
        }
        self.next_generation = last;
        self.initialized = true;
        Ok(result)
    }

    pub fn capture(&self, segs: SmallVec<[PathSeg; 5]>) -> ContentPath {
        let mut path = ContentPath::new(segs, self.current);
        path.binding = Some(PathBinding {
            origin: self.origin.clone(),
            states: smallvec::smallvec![self.document.clone()],
        });
        path
    }

    pub fn capture_scopes(
        &self,
        segs: SmallVec<[PathSeg; 5]>,
        scopes: &[RevisionState],
    ) -> Result<ContentPath, RevisionError> {
        if scopes.iter().any(|state| !self.owns(state)) {
            return Err(RevisionError::ForeignOwner);
        }
        if scopes.iter().any(|state| state.first_cause().is_some()) {
            return Err(RevisionError::Retired);
        }
        let mut states = SmallVec::new();
        states
            .try_reserve(scopes.len())
            .map_err(|_| RevisionError::Allocation)?;
        states.extend(scopes.iter().cloned());
        let mut path = ContentPath::new(segs, self.current);
        path.binding = Some(PathBinding {
            origin: self.origin.clone(),
            states,
        });
        Ok(path)
    }

    /// Capture an already allocated future document state before allocating a
    /// returned Python handle. Failure leaves the current counter unchanged.
    pub fn capture_prepared(
        &self,
        prepared: &PreparedRevision,
        segs: SmallVec<[PathSeg; 5]>,
    ) -> Result<ContentPath, RevisionError> {
        if !std::sync::Arc::ptr_eq(&self.origin, &prepared.origin) {
            return Err(RevisionError::ForeignOwner);
        }
        if self.current != prepared.expected_serial
            || self.next_generation != prepared.expected_generation
        {
            return Err(RevisionError::StalePreparation);
        }
        let mut path = ContentPath::new(segs, prepared.committed_serial());
        path.binding = Some(PathBinding {
            origin: self.origin.clone(),
            states: smallvec::smallvec![prepared.successors[0].clone()],
        });
        Ok(path)
    }

    /// Allocate all affected successors and new owner scopes before native publication.
    pub fn prepare_scopes(
        &self,
        operation: &'static str,
        scopes: &[RevisionState],
        born: &[RevisionScope],
    ) -> Result<PreparedRevision, RevisionError> {
        self.prepare_revision(Some(operation), scopes, born, RevisionPreparation::Global)
    }

    /// Advance the commit serial while retiring only the supplied owner scopes.
    /// Captured document authority stays live for checked local replacements.
    pub fn prepare_local_scopes(
        &self,
        operation: &'static str,
        scopes: &[RevisionState],
        born: &[RevisionScope],
    ) -> Result<PreparedRevision, RevisionError> {
        self.prepare_revision(Some(operation), scopes, born, RevisionPreparation::Local)
    }

    /// Allocate the document state and births under the selected publication policy.
    /// Replacement reserves the same generation positions as Global but returns no
    /// owner successors. Every mode validates all retirements before publication.
    pub fn prepare_revision(
        &self,
        operation: Option<&'static str>,
        scopes: &[RevisionState],
        born: &[RevisionScope],
        mode: RevisionPreparation,
    ) -> Result<PreparedRevision, RevisionError> {
        let retire_document = mode != RevisionPreparation::Local;
        let owner_successors = mode != RevisionPreparation::Replacement;
        self.current.checked_add(1).ok_or(RevisionError::Overflow)?;
        let total = scopes
            .len()
            .checked_add(born.len())
            .and_then(|n| n.checked_add(usize::from(retire_document)))
            .ok_or(RevisionError::Overflow)?;
        let next_generation = self
            .next_generation
            .checked_add(u64::try_from(total).map_err(|_| RevisionError::Overflow)?)
            .ok_or(RevisionError::Overflow)?;
        let mut retired = Vec::new();
        retired
            .try_reserve_exact(scopes.len() + 1)
            .map_err(|_| RevisionError::Allocation)?;
        retired.push(self.document.clone());
        // Identity is the shared allocation, as in `same_state`. A set keeps a
        // whole-inventory retirement linear in the number of scopes.
        let mut seen = std::collections::HashSet::<
            *const ScopeGeneration,
            std::hash::BuildHasherDefault<AddressHasher>,
        >::default();
        seen.try_reserve(scopes.len() + 1)
            .map_err(|_| RevisionError::Allocation)?;
        seen.insert(std::sync::Arc::as_ptr(&self.document.0));
        for scope in scopes {
            if !self.owns(scope) {
                return Err(RevisionError::ForeignOwner);
            }
            if scope.first_cause().is_some() {
                return Err(RevisionError::Retired);
            }
            if !seen.insert(std::sync::Arc::as_ptr(&scope.0)) {
                return Err(RevisionError::DuplicateScope);
            }
            retired.push(scope.clone());
        }
        let mut successors = Vec::new();
        successors
            .try_reserve_exact(if owner_successors { retired.len() } else { 1 })
            .map_err(|_| RevisionError::Allocation)?;
        let mut generation = self.next_generation;
        for (index, old) in retired.iter().enumerate() {
            if index == 0 && !retire_document {
                successors.push(old.clone());
                continue;
            }
            generation += 1;
            if index != 0 && !owner_successors {
                continue;
            }
            successors.push(self.state(generation, old.scope()));
        }
        let mut births = Vec::new();
        births
            .try_reserve_exact(born.len())
            .map_err(|_| RevisionError::Allocation)?;
        for scope in born {
            generation += 1;
            births.push(self.state(generation, *scope));
        }
        Ok(PreparedRevision {
            origin: self.origin.clone(),
            expected_serial: self.current,
            expected_generation: self.next_generation,
            next_generation,
            operation,
            retire_document,
            retired,
            successors,
            born: births,
        })
    }

    /// Under the same exclusive wrapper borrow, commit before infallible native swaps.
    /// All rejection is before the first retirement. No fallible action follows success.
    pub fn commit_scopes(&mut self, prepared: PreparedRevision) -> Result<u64, RevisionError> {
        if !std::sync::Arc::ptr_eq(&self.origin, &prepared.origin) {
            return Err(RevisionError::ForeignOwner);
        }
        if self.current != prepared.expected_serial
            || self.next_generation != prepared.expected_generation
            || !self.document.same_state(&prepared.retired[0])
        {
            return Err(RevisionError::StalePreparation);
        }
        if prepared
            .retired
            .iter()
            .any(|state| state.first_cause().is_some())
        {
            return Err(RevisionError::Retired);
        }
        let serial = self.current + 1;
        for state in prepared
            .retired
            .iter()
            .skip(usize::from(!prepared.retire_document))
        {
            state
                .0
                .retired
                .set(InvalidationCause {
                    serial,
                    operation: prepared.operation,
                    scope: state.scope(),
                })
                .expect("private retirement under the owning exclusive borrow was prevalidated");
        }
        self.current = serial;
        self.next_generation = prepared.next_generation;
        self.document = prepared.successors[0].clone();
        Ok(serial)
    }

    /// Numeric compatibility. Migrated consumers prepare with the real public label.
    pub fn bump(&mut self) -> u64 {
        let prepared = self
            .prepare_revision(None, &[], &[], RevisionPreparation::Global)
            .expect("document revision counter overflowed");
        self.commit_scopes(prepared)
            .expect("legacy numeric transaction was just prepared")
    }
}

impl PartialEq for ContentPath {
    fn eq(&self, other: &Self) -> bool {
        self.segs == other.segs
            && self.revision == other.revision
            && self.text_revision == other.text_revision
    }
}
impl Eq for ContentPath {}

impl ContentPath {
    /// Unbound numeric selectors remain current-document explicit addressing.
    pub fn belongs_to(&self, counter: &RevisionCounter) -> bool {
        self.binding
            .as_ref()
            .is_none_or(|binding| std::sync::Arc::ptr_eq(&binding.origin, &counter.origin))
    }

    /// The physical owner is resolved by its checked generation, never visible equality.
    pub fn owner_generation(&self) -> Option<u64> {
        self.binding
            .as_ref()?
            .states
            .iter()
            .rev()
            .find(|state| state.scope() == RevisionScope::Owner)
            .map(RevisionState::generation)
    }

    /// Select the earliest committed retirement, with fixed scope ordering for ties.
    pub fn validate_scopes(
        &self,
        counter: &RevisionCounter,
        kind: &str,
        hint: &str,
    ) -> Result<(), StaleElementError> {
        if !self.belongs_to(counter) {
            return Err(StaleElementError {
                element_kind: kind.to_owned(),
                captured_revision: self.revision,
                current_revision: counter.current(),
                recovery_hint: "belongs to a different document".to_owned(),
            });
        }
        let Some(binding) = &self.binding else {
            return self.validate_revision(
                counter.current(),
                kind,
                &format!(
                    "Causal provenance is unavailable for this explicit numeric selector. {hint}"
                ),
            );
        };
        let first = binding
            .states
            .iter()
            .filter_map(RevisionState::first_cause)
            .min_by_key(|cause| (cause.serial, cause.scope));
        let Some(cause) = first else {
            return Ok(());
        };
        let reason = cause.operation.map_or_else(
            || "Causal provenance is unavailable for this legacy mutation.".to_owned(),
            |operation| {
                format!(
                    "First invalidated by {operation} at committed revision {}.",
                    cause.serial
                )
            },
        );
        Err(StaleElementError {
            element_kind: kind.to_owned(),
            captured_revision: self.revision,
            current_revision: counter.current(),
            recovery_hint: format!("{reason} {hint}"),
        })
    }
}

/// Convert inches to EMU with the canonical truncation behavior.
pub fn emu_from_inches(value: f64) -> i64 {
    Length::inches(value).to_emu()
}

/// Convert centimetres to EMU with the canonical truncation behavior.
pub fn emu_from_centimetres(value: f64) -> i64 {
    Length::cm(value).to_emu()
}

/// Convert millimetres to EMU with the canonical truncation behavior.
pub fn emu_from_millimetres(value: f64) -> i64 {
    Length::mm(value).to_emu()
}

/// Convert points to EMU with the canonical truncation behavior.
pub fn emu_from_points(value: f64) -> i64 {
    Length::pt(value).to_emu()
}

/// Convert twips to EMU.
pub fn emu_from_twips(value: i32) -> i64 {
    Length::twips(value).to_emu()
}

/// Convert EMU to inches.
pub fn inches_from_emu(value: i64) -> f64 {
    Length::emu(value).to_inches()
}

/// Convert EMU to centimetres.
pub fn centimetres_from_emu(value: i64) -> f64 {
    Length::emu(value).to_cm()
}

/// Convert EMU to millimetres.
pub fn millimetres_from_emu(value: i64) -> f64 {
    Length::emu(value).to_cm() * 10.0
}

/// Convert EMU to points.
pub fn points_from_emu(value: i64) -> f64 {
    Length::emu(value).to_pt()
}

/// Convert EMU to twips.
pub fn twips_from_emu(value: i64) -> i32 {
    Length::emu(value).to_twips()
}

/// Convert a native signature report to independent Python dictionaries and lists.
pub fn signature_report_to_py(
    py: Python<'_>,
    report: &oxoffice_opc::SignatureReport,
) -> PyResult<Py<pyo3::types::PyDict>> {
    use oxoffice_opc::SignatureIssue;
    use pyo3::types::{PyDict, PyList};
    let result = PyDict::new(py);
    result.set_item("signature_part", &report.signature_part)?;
    if let Some(signer) = &report.signer {
        let identity = PyDict::new(py);
        identity.set_item("subject", &signer.subject)?;
        identity.set_item("issuer", &signer.issuer)?;
        identity.set_item("serial_number", &signer.serial_number)?;
        result.set_item("signer", identity)?;
    } else {
        result.set_item("signer", py.None())?;
    }
    result.set_item("cryptographically_valid", report.cryptographically_valid)?;
    result.set_item("coverage_complete", report.coverage_complete)?;
    result.set_item("covered_parts", &report.covered_parts)?;
    let relationships = PyList::empty(py);
    for relationship in &report.covered_relationships {
        let item = PyDict::new(py);
        item.set_item("source_part", &relationship.source_part)?;
        item.set_item("relationship_id", &relationship.relationship_id)?;
        relationships.append(item)?;
    }
    result.set_item("covered_relationships", relationships)?;
    let issues = PyList::empty(py);
    for issue in &report.issues {
        let item = PyDict::new(py);
        let details = PyDict::new(py);
        let kind = match issue {
            SignatureIssue::MissingPart { uri } => {
                details.set_item("uri", uri)?;
                "missing_part"
            }
            SignatureIssue::ChangedReference {
                uri,
                expected_digest,
                actual_digest,
            } => {
                details.set_item("uri", uri)?;
                details.set_item("expected_digest", expected_digest)?;
                details.set_item("actual_digest", actual_digest)?;
                "changed_reference"
            }
            SignatureIssue::DuplicateReference { uri } => {
                details.set_item("uri", uri)?;
                "duplicate_reference"
            }
            SignatureIssue::MissingRelationship {
                source_part,
                relationship_id,
            } => {
                details.set_item("source_part", source_part)?;
                details.set_item("relationship_id", relationship_id)?;
                "missing_relationship"
            }
            SignatureIssue::DuplicateRelationship {
                source_part,
                relationship_id,
            } => {
                details.set_item("source_part", source_part)?;
                details.set_item("relationship_id", relationship_id)?;
                "duplicate_relationship"
            }
            SignatureIssue::ExternalRelationship {
                source_part,
                relationship_id,
                target,
            } => {
                details.set_item("source_part", source_part)?;
                details.set_item("relationship_id", relationship_id)?;
                details.set_item("target", target)?;
                "external_relationship"
            }
            SignatureIssue::MissingRelationshipTarget {
                source_part,
                relationship_id,
                target,
            } => {
                details.set_item("source_part", source_part)?;
                details.set_item("relationship_id", relationship_id)?;
                details.set_item("target", target)?;
                "missing_relationship_target"
            }
            SignatureIssue::UncoveredPart { part_name } => {
                details.set_item("part_name", part_name)?;
                "uncovered_part"
            }
            SignatureIssue::UncoveredRelationship {
                source_part,
                relationship_id,
            } => {
                details.set_item("source_part", source_part)?;
                details.set_item("relationship_id", relationship_id)?;
                "uncovered_relationship"
            }
            SignatureIssue::SignatureValueMismatch => "signature_value_mismatch",
            SignatureIssue::OrphanSignaturePart { part_name } => {
                details.set_item("part_name", part_name)?;
                "orphan_signature_part"
            }
        };
        item.set_item("kind", kind)?;
        item.set_item("details", details)?;
        issues.append(item)?;
    }
    result.set_item("issues", issues)?;
    Ok(result.unbind())
}

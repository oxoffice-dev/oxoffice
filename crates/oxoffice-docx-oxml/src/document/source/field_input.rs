//! Field inputs: the source of a field's instruction and result.

use super::*;

impl WordFieldInput {
    pub(crate) fn produced(
        map: WordSourceMap,
        registry: std::sync::Arc<std::sync::Mutex<crate::text::FieldOriginRegistry>>,
    ) -> Self {
        Self {
            initial_delivery: false,
            map: std::sync::Arc::new(map),
            registry,
        }
    }

    /// Bind the real immutable capture record as another CURRENT-coordinate
    /// domain. Keep the returned numeric collector live through its constructor
    /// and freeze its roles before returning a map beyond that record scope.
    /// Existing A correspondence and first-delivery authority are unchanged.
    pub(crate) fn record_publication(
        &self,
        raw: &[u8],
    ) -> Result<(
        Self,
        std::sync::Arc<std::sync::Mutex<WordPublicationCoverage>>,
    )> {
        if raw.len() != self.map.len() {
            return Err(OxmlError::InvalidValue(
                "publication capture record length changed".into(),
            ));
        }
        let collector =
            std::sync::Arc::new(std::sync::Mutex::new(WordPublicationCoverage::default()));
        let mut map = (*self.map).clone();
        map.publication.push(WordPublicationPiece {
            output: 0..raw.len(),
            current: 0..raw.len(),
            collector: std::sync::Arc::downgrade(&collector),
        });
        Ok((
            Self {
                initial_delivery: self.initial_delivery,
                map: std::sync::Arc::new(map),
                registry: std::sync::Arc::clone(&self.registry),
            },
            collector,
        ))
    }

    pub(crate) fn property_observation(
        &self,
        selected: std::ops::Range<usize>,
    ) -> Option<WordOrdinaryFact> {
        (selected.start < selected.end && selected.end <= self.map.len()).then(|| {
            WordOrdinaryFact {
                map: std::sync::Arc::clone(&self.map),
                selected,
            }
        })
    }

    pub(crate) fn registry(
        &self,
    ) -> &std::sync::Arc<std::sync::Mutex<crate::text::FieldOriginRegistry>> {
        &self.registry
    }

    pub(crate) fn resource_failure(&self) -> Option<OxmlError> {
        self.registry
            .lock()
            .expect("field origin registry poisoned")
            .source_resource_failure()
    }

    pub(crate) fn projection_parts(
        &self,
    ) -> (
        &WordSourceMap,
        std::sync::Arc<std::sync::Mutex<crate::text::FieldOriginRegistry>>,
        bool,
    ) {
        (
            &self.map,
            std::sync::Arc::clone(&self.registry),
            self.initial_delivery,
        )
    }

    pub(crate) fn slice(&self, range: std::ops::Range<usize>) -> Result<Self> {
        Ok(Self {
            initial_delivery: self.initial_delivery,
            map: std::sync::Arc::new(self.map.slice(range)?),
            registry: std::sync::Arc::clone(&self.registry),
        })
    }

    pub(crate) fn with_shell(&self, prefix: usize, suffix: usize) -> Result<Self> {
        Ok(Self {
            initial_delivery: self.initial_delivery,
            map: std::sync::Arc::new(WordSourceMap::concat(&[
                WordSourceMap::generated(prefix),
                (*self.map).clone(),
                WordSourceMap::generated(suffix),
            ])?),
            registry: std::sync::Arc::clone(&self.registry),
        })
    }

    /// Preserve this actual constructor input through its namespace closure.
    /// The flag comes only from this input, never from its Original pieces.
    pub(crate) fn close_namespaces(
        &self,
        raw: &[u8],
        bindings: &[(String, String)],
    ) -> Result<(Vec<u8>, Self)> {
        if self.map.len() != raw.len() {
            return Err(OxmlError::InvalidValue(
                "mapped capture input length changed".into(),
            ));
        }
        let (bytes, map) = crate::text::raw_with_external_bindings_and_map(
            raw,
            bindings,
            None,
            Some(std::sync::Arc::clone(&self.map)),
        )?;
        // The closure's concrete insertion transcript normally preserves Some.
        // An unproved map stays Generated and cannot supply input authority.
        let map = map.unwrap_or_else(|| std::sync::Arc::new(WordSourceMap::generated(bytes.len())));
        if map.len() != bytes.len() {
            return Err(OxmlError::InvalidValue(
                "mapped capture output length changed".into(),
            ));
        }
        Ok((
            bytes,
            Self {
                initial_delivery: self.initial_delivery,
                map,
                registry: std::sync::Arc::clone(&self.registry),
            },
        ))
    }

    pub(crate) fn cursor<'a>(&self, input: &'a [u8]) -> Result<ControlSourceCursor<'a>> {
        if self.map.len() != input.len() {
            return Err(OxmlError::InvalidValue(
                "mapped field reader input length changed".into(),
            ));
        }
        Ok(ControlSourceCursor {
            occurrences: &[],
            next: 0,
            end: 0,
            projected_aliases: false,
            initial_field_delivery: self.initial_delivery,
            simple_headers: &[],
            simple_selectors: &[],
            selector_paragraph_root: None,
            selector_requested: false,
            selector_ready: false,
            input: Some(input),
            source_map: std::sync::Arc::clone(&self.map),
            field_origins: Some(std::sync::Arc::clone(&self.registry)),
        })
    }

    pub(crate) fn register(&self, field: &crate::text::Field) -> Result<()> {
        self.registry
            .lock()
            .expect("field origin registry poisoned")
            .register_initial(field, (*self.map).clone(), self.initial_delivery)
    }
}

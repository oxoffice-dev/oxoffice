/// Raw XML children grouped by their schema boundary.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OrderedRawChildren {
    children: Vec<RawChild>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RawChild {
    boundary: usize,
    xml: Vec<u8>,
}

impl OrderedRawChildren {
    /// Records a raw child at a caller-defined schema boundary.
    pub fn push(&mut self, boundary: usize, raw_xml: Vec<u8>) {
        self.children.push(RawChild {
            boundary,
            xml: raw_xml,
        });
    }

    /// Returns raw children recorded at a schema boundary.
    pub fn at(&self, boundary: usize) -> impl Iterator<Item = &[u8]> {
        self.children
            .iter()
            .filter(move |child| child.boundary == boundary)
            .map(|child| child.xml.as_slice())
    }

    /// Returns whether no raw children have been recorded.
    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }

    /// Returns every preserved child in recording order.
    pub fn iter(&self) -> impl Iterator<Item = &[u8]> {
        self.children.iter().map(|child| child.xml.as_slice())
    }

    /// Keeps only the raw children whose XML satisfies `keep`.
    pub fn retain(&mut self, mut keep: impl FnMut(&[u8]) -> bool) {
        self.children.retain(|child| keep(&child.xml));
    }

    /// Moves raw children at and after one boundary to make room for a new
    /// modelled child without changing their relative order.
    pub fn shift_boundaries_from(&mut self, boundary: usize) {
        for child in &mut self.children {
            if child.boundary >= boundary {
                child.boundary += 1;
            }
        }
    }

    /// Returns raw children at their effective boundary after edits to a
    /// public collection. Each original boundary is anchored to the next
    /// surviving original item, or to the trailing boundary when none remains.
    pub fn at_reconciled<'a>(
        &'a self,
        boundary: usize,
        offset: usize,
        original_to_current: &'a [Option<usize>],
        current_len: usize,
    ) -> impl Iterator<Item = &'a [u8]> {
        self.children
            .iter()
            .filter(move |child| {
                if child.boundary < offset {
                    return false;
                }
                let original_index = child.boundary - offset;
                let effective = original_to_current
                    .iter()
                    .skip(original_index)
                    .flatten()
                    .copied()
                    .next()
                    .unwrap_or(current_len);
                effective == boundary
            })
            .map(|child| child.xml.as_slice())
    }
}

impl OrderedRawChildren {
    /// Folds the removed typed member boundary without losing raw sibling bytes.
    pub fn close_boundary(&mut self, boundary: usize) {
        for child in &mut self.children {
            if child.boundary >= boundary && child.boundary > 0 {
                child.boundary -= 1;
            }
        }
    }
}

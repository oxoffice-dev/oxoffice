//! The bytes a Word admission reads.

use super::*;

/// A concrete byte source for original or explicitly planned source pieces.
/// Piece order is the producer's occurrence order. No output Vec is required
/// to run the actual parser/admission decisions.
#[derive(Clone, Copy)]
pub(crate) enum WordXmlInput<'a> {
    Contiguous(&'a [u8]),
    Pieces {
        chunks: &'a [&'a [u8]],
        index: usize,
        offset: usize,
    },
}
impl<'a> WordXmlInput<'a> {
    pub(crate) fn pieces(chunks: &'a [&'a [u8]]) -> Self {
        Self::Pieces {
            chunks,
            index: 0,
            offset: 0,
        }
    }
    pub(super) fn len(self) -> Result<usize> {
        let mut total = 0usize;
        for chunk in self.chunks() {
            total = total
                .checked_add(chunk.len())
                .ok_or_else(|| OxmlError::InvalidValue("Word source byte overflow".into()))?;
        }
        Ok(total)
    }
    pub(super) fn chunks(self) -> Vec<&'a [u8]> {
        match self {
            Self::Contiguous(raw) => vec![raw],
            Self::Pieces {
                chunks,
                index,
                offset,
            } => chunks
                .iter()
                .enumerate()
                .skip(index)
                .map(|(at, chunk)| {
                    if at == index {
                        &chunk[offset..]
                    } else {
                        *chunk
                    }
                })
                .collect(),
        }
    }
}
impl std::io::Read for WordXmlInput<'_> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        let input = std::io::BufRead::fill_buf(self)?;
        let bytes = input.len().min(output.len());
        output[..bytes].copy_from_slice(&input[..bytes]);
        std::io::BufRead::consume(self, bytes);
        Ok(bytes)
    }
}
impl std::io::BufRead for WordXmlInput<'_> {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        match self {
            Self::Contiguous(raw) => Ok(raw),
            Self::Pieces {
                chunks,
                index,
                offset,
            } => {
                while *index < chunks.len() && *offset == chunks[*index].len() {
                    *index += 1;
                    *offset = 0;
                }
                Ok(chunks.get(*index).map_or(&[], |chunk| &chunk[*offset..]))
            }
        }
    }
    fn consume(&mut self, bytes: usize) {
        match self {
            Self::Contiguous(raw) => *raw = &raw[bytes.min(raw.len())..],
            Self::Pieces {
                chunks,
                index,
                offset,
            } => {
                if let Some(chunk) = chunks.get(*index) {
                    *offset += bytes.min(chunk.len() - *offset);
                }
            }
        }
    }
}

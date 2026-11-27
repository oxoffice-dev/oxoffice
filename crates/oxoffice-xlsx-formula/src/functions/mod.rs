//! The built-in worksheet functions: the prefix each is stored with and how
//! many arguments it takes.
//!
//! The table is written by `tools/gen-xlsx-functions/generate.py` from
//! MS-XLSX section 2.2.2 (its worksheet functions, the argument rule of each,
//! and its future-function table), cross-checked with the MS-XLS function
//! table; the script names the rules it corrects and the later functions it
//! adds.

mod table;

use table::FUNCTIONS;

/// What a built-in function's name is stored after.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Prefix {
    /// Stored as displayed: `SUM`.
    None,
    /// A future function: `_xlfn.XLOOKUP`.
    Xlfn,
    /// A future function Excel limits to worksheets: `_xlfn._xlws.FILTER`.
    XlfnXlws,
}

impl Prefix {
    /// The text written before the name.
    pub fn as_str(self) -> &'static str {
        match self {
            Prefix::None => "",
            Prefix::Xlfn => "_xlfn.",
            Prefix::XlfnXlws => "_xlfn._xlws.",
        }
    }
}

/// A built-in worksheet function.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Function {
    /// The name as displayed, in capitals: `XLOOKUP`.
    pub name: &'static str,
    pub prefix: Prefix,
    pub min_arguments: u8,
    pub max_arguments: u8,
}

impl Function {
    const fn new(name: &'static str, prefix: Prefix, min: u8, max: u8) -> Self {
        Function {
            name,
            prefix,
            min_arguments: min,
            max_arguments: max,
        }
    }
}

/// The built-in function of this displayed name (`xlookup`, any case), or of
/// this stored name when its prefix is the one the function is stored with
/// (`_xlfn.XLOOKUP`). `None` for any other name: a function a workbook or an
/// add-in defines, or a prefixed name no table lists.
pub fn function(name: &str) -> Option<&'static Function> {
    let (prefix, bare) = split_prefix(name);
    let found = lookup(bare)?;
    (prefix.is_empty() || prefix == found.prefix.as_str()).then_some(found)
}

/// The function stored under exactly this text, prefix included: `None` for
/// `XLOOKUP` (Excel stores `_xlfn.XLOOKUP`) as for an unknown name.
pub(crate) fn stored_function(name: &str) -> Option<&'static Function> {
    let (prefix, bare) = split_prefix(name);
    lookup(bare).filter(|found| found.prefix.as_str() == prefix)
}

/// `_xlfn.` and `_xlfn._xlws.` split from the name, as written (lower case).
fn split_prefix(name: &str) -> (&str, &str) {
    for prefix in ["_xlfn._xlws.", "_xlfn."] {
        if let Some(bare) = name.strip_prefix(prefix) {
            return (prefix, bare);
        }
    }
    ("", name)
}

fn lookup(bare: &str) -> Option<&'static Function> {
    if bare.len() > 32 || !bare.is_ascii() {
        return None;
    }
    let upper = bare.to_ascii_uppercase();
    FUNCTIONS
        .binary_search_by(|function| function.name.cmp(upper.as_str()))
        .ok()
        .map(|index| &FUNCTIONS[index])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_sorted_and_every_range_is_sound() {
        for pair in FUNCTIONS.windows(2) {
            assert!(
                pair[0].name < pair[1].name,
                "{} {}",
                pair[0].name,
                pair[1].name
            );
        }
        for function in FUNCTIONS {
            assert!(
                function.min_arguments <= function.max_arguments,
                "{}",
                function.name
            );
            assert!(function.name.len() <= 32, "{}", function.name);
        }
    }
}

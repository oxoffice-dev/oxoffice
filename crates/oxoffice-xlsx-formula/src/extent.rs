//! How large a formula's result is, read from its text without an evaluator:
//! what a dynamic array formula spills over.
//!
//! A range gives its rows and columns; an operator, or a function applied to
//! each element of its arguments, gives the shape of its largest operand, a
//! dimension of 1 repeated; the functions that build arrays (`SORT`,
//! `TRANSPOSE`, `SEQUENCE`, `TAKE`, `HSTACK`...) give theirs from the shapes
//! and constants they take. A function whose result the data size (`FILTER`,
//! `UNIQUE`, `TAKE` by a count read from a cell) gives the most it can hold,
//! its input's shape. A size read from a cell (`SEQUENCE(B1)`), a function
//! the table does not list, a table reference or a LAMBDA leaves the extent
//! unknown.
//!
//! An error that replaces the whole result is one cell, within any extent:
//! a value that can only make one (a text where `SEQUENCE` takes its start,
//! an error as an `IF` condition) is not looked at.

use std::collections::HashMap;

use crate::forms::parameter;
use crate::functions::function;
use crate::lexer::TokenKind;
use crate::parser::{Node, Parsed, parse};
use crate::reference::{MAX_COLUMNS, MAX_ROWS, Part, Target, target, target_start};

/// The rows and columns of a result: 1 by 1 for one value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shape {
    pub rows: u32,
    pub columns: u32,
}

impl Shape {
    /// One value.
    pub const ONE: Shape = Shape {
        rows: 1,
        columns: 1,
    };

    /// A shape a worksheet holds: 1 to 1,048,576 rows, 1 to 16,384 columns.
    fn new(rows: u64, columns: u64) -> Option<Shape> {
        let rows = u32::try_from(rows)
            .ok()
            .filter(|rows| (1..=MAX_ROWS).contains(rows))?;
        let columns = u32::try_from(columns)
            .ok()
            .filter(|columns| (1..=MAX_COLUMNS).contains(columns))?;
        Some(Shape { rows, columns })
    }
}

/// How large a formula's result is: its shape, or the largest it can have.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Extent {
    pub shape: Shape,
    /// Whether the result has that shape; else the data decide how much of
    /// it, from its first cell, the result fills (`FILTER`, `UNIQUE`).
    pub exact: bool,
    /// The part of it the data fill: past it every element is what a blank
    /// cell gives. Whole columns hold their sheet's used rows and one for
    /// the blank cells after them; `None` when that is not known (whole
    /// columns of another workbook).
    data: Option<Shape>,
}

impl Extent {
    fn exact(shape: Shape) -> Extent {
        Extent::new(shape, true)
    }

    /// At most `shape`, or exactly when that is known: one value is one
    /// value.
    fn new(shape: Shape, exact: bool) -> Extent {
        Extent {
            shape,
            exact: exact || shape == Shape::ONE,
            data: Some(shape),
        }
    }

    /// The same extent with the part the data fill.
    fn with_data(self, data: Option<Shape>) -> Extent {
        Extent { data, ..self }
    }

    /// Whether `self` and `other` say the same of a result.
    fn same(self, other: Extent) -> bool {
        self.shape == other.shape && self.exact == other.exact
    }

    fn of(rows: u64, columns: u64, exact: bool) -> Option<Extent> {
        Some(Extent::new(Shape::new(rows, columns)?, exact))
    }

    fn rows(self) -> u64 {
        u64::from(self.shape.rows)
    }

    fn columns(self) -> u64 {
        u64::from(self.shape.columns)
    }
}

/// How many defined names deep a formula is followed: a name that refers to
/// itself, directly or not, stops there.
const MAX_NAME_DEPTH: usize = 4;

/// The operators applied to each element of their operands.
const ELEMENT_OPERATORS: [&str; 12] = [
    "+", "-", "*", "/", "^", "&", "=", "<>", "<", "<=", ">", ">=",
];

/// How large the result of `formula` (stored or displayed, without its `=`)
/// is, when its text tells it, or `None`.
///
/// Exactly:
///
/// - a constant is one value, an array constant its rows and columns, a
///   range its own (whole columns hold 1,048,576 rows), a defined name the
///   extent of what it refers to, a LET name that of its value;
/// - an operator, and a function applied to each element of its arguments
///   (`ROUND`, `LEN`, `IFERROR`, the criteria of `COUNTIF`, the value
///   `MATCH` looks up), gives the shape of its largest operand, a dimension
///   of 1 repeated (two that differ in a dimension neither has at 1 give at
///   most the larger);
/// - an aggregate (`SUM`, `AND`, `TEXTJOIN`, `ROWS`) is one value; `SORT`
///   and `SORTBY` keep the extent of what they sort, `TRANSPOSE` swaps it,
///   `TOCOL` and `TOROW` line it up, `HSTACK` and `VSTACK` join theirs,
///   `MMULT` multiplies, `ROW` and `COLUMN` give one per row or column;
/// - `SEQUENCE`, `RANDARRAY`, `MAKEARRAY` and `MUNIT` take their size,
///   `TAKE`, `DROP`, `EXPAND`, `CHOOSECOLS`, `CHOOSEROWS`, `WRAPROWS` and
///   `WRAPCOLS` their counts, from number constants (or `ROWS` and
///   `COLUMNS` of an exact extent);
/// - `IF` and `CHOOSE` on an array give the largest of their arguments.
///
/// At most, the data deciding how much of it the result fills: `FILTER`
/// (what it filters, or `if_empty`; not whole columns or rows, whose blank
/// rows may each pass), `UNIQUE` (what it reads, whole columns or rows as
/// far as their sheet's used range and one more for the blank cells after
/// it), `FREQUENCY`
/// (one more than its bins), `TAKE` and `DROP` by a count, and `CHOOSECOLS`
/// or `CHOOSEROWS` by an index, read from a cell, a `TOCOL` or `TOROW` that
/// skips, `IF` and `CHOOSE` on one value (the largest of their choices), and
/// whatever takes the result of one of them; not when that bound would be
/// whole columns or rows (Google Sheets keeps a grid that large).
///
/// `None` for a size read from a cell (`SEQUENCE(B1)`, `EXPAND(A1:B2,B1)`),
/// a function this table does not list (`GROUPBY`, `TEXTSPLIT`, `XLOOKUP`,
/// `INDEX`, `OFFSET`...), a table reference, a 3D reference, a LAMBDA, the
/// spill of another cell (`J2#`), a union or intersection of references, a
/// result no worksheet holds, and a formula this crate cannot read.
///
/// ```
/// use oxoffice_xlsx_formula::{DefinedName, Lookups, NameScope, result_extent};
///
/// let names = |_: NameScope<'_>, name: &str| {
///     (name == "Sales").then(|| DefinedName { formula: "Data!$B$2:$B$25".to_owned(), sheet: None })
/// };
/// // The formula's sheet holds values down to row 40, in columns A to C.
/// let used = |sheet: Option<&str>| sheet.is_none().then_some((40, 3));
/// let lookups = Lookups { names: &names, used: &used };
/// let extent = |formula| {
///     result_extent(formula, &lookups).map(|e| (e.shape.rows, e.shape.columns, e.exact))
/// };
/// assert_eq!(extent("_xlfn._xlws.SORT(Sales,,-1)"), Some((24, 1, true)));
/// assert_eq!(extent("ROUND(A2:C10*2,1)"), Some((9, 3, true)));
/// assert_eq!(extent("_xlfn.SEQUENCE(3,4)"), Some((3, 4, true)));
/// assert_eq!(extent("_xlfn.UNIQUE(A2:B10)"), Some((9, 2, false)));
/// assert_eq!(extent("_xlfn.UNIQUE(A:A)"), Some((41, 1, false)));
/// assert_eq!(extent("_xlfn.SEQUENCE(B1)"), None);
/// ```
pub fn result_extent(formula: &str, lookups: &Lookups<'_>) -> Option<Extent> {
    let mut shared = Shared::default();
    let extent = extent_of(formula, lookups, 0, &Within::Formula, &mut shared)?;
    let cells = u64::from(extent.shape.rows) * u64::from(extent.shape.columns);
    let bounded = extent.exact
        || (extent.shape.rows < MAX_ROWS
            && extent.shape.columns < MAX_COLUMNS
            && cells <= MAX_BOUND_CELLS);
    (bounded && shared.nodes <= MAX_NODES).then_some(extent)
}

/// The most cells a bound covers: a wider one is no bound (Google Sheets
/// keeps a grid as large as the `ref`, and holds 10 million cells).
const MAX_BOUND_CELLS: u64 = 1_000_000;
/// The most parts of formulas, defined names' included, one extent reads:
/// a workbook whose names nest wide and deep leaves it unknown.
const MAX_NODES: usize = 100_000;

/// Where a defined name is looked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameScope<'a> {
    /// In the formula of a cell: its sheet's own names, then the workbook's.
    Formula,
    /// Written after a sheet (`Data!Sales`): that sheet's own names.
    Sheet(&'a str),
    /// In the formula of a sheet's own name: that sheet's names, then the
    /// workbook's.
    WithinSheet(&'a str),
    /// In the formula of a workbook name: the workbook's names.
    Workbook,
}

/// A defined name: what it refers to, and the sheet it belongs to (`None`
/// for a workbook name), whose names its own formula reads first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DefinedName {
    pub formula: String,
    pub sheet: Option<String>,
}

/// The defined name of a name: see [`Lookups::names`].
pub type NameLookup<'a> = dyn Fn(NameScope<'_>, &str) -> Option<DefinedName> + 'a;
/// How far a sheet's cells reach: see [`Lookups::used`].
pub type UsedLookup<'a> = dyn Fn(Option<&str>) -> Option<(u32, u32)> + 'a;

/// What [`result_extent`] reads from the workbook.
pub struct Lookups<'a> {
    /// The defined name of a name, looked for in a scope (a sheet written
    /// before it, `Data!Sales`, is unquoted); `None` for a name the scope
    /// does not define.
    pub names: &'a NameLookup<'a>,
    /// The last row and column a sheet's values and formulas reach, 1-based,
    /// 0 for a sheet without any: the formula's own sheet for `None`, else
    /// the sheet named, unquoted; `None` for a sheet the workbook lacks.
    pub used: &'a UsedLookup<'a>,
}

/// Whose names the formula being read reads first.
#[derive(Clone, Debug)]
enum Within {
    Formula,
    Sheet(String),
    Workbook,
}

/// What one extent's reading shares across the defined names it follows.
#[derive(Default)]
struct Shared {
    /// The extent of each name followed, by scope and name; `None` while it
    /// is read, so a name that refers to itself stops there.
    names: HashMap<String, Option<Extent>>,
    /// The parts of formulas read.
    nodes: usize,
}

fn extent_of(
    formula: &str,
    lookups: &Lookups<'_>,
    depth: usize,
    within: &Within,
    shared: &mut Shared,
) -> Option<Extent> {
    let parsed = parse(formula, false).ok()?;
    let mut inference = Inference {
        parsed: &parsed,
        lookups,
        depth,
        within,
        shared,
        scope: Vec::new(),
    };
    inference.extent(&parsed.root)
}

/// How a function's result takes its extent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rule {
    /// One value whatever the arguments.
    One,
    /// Applied to each element of the arguments the pattern marks `v`, the
    /// arguments marked `a` taken whole; the letters after `|` repeat.
    Each(&'static str),
    /// The extent of the first argument: `SORT`, `SORTBY`.
    First,
    /// At most the extent of the first argument: `UNIQUE`.
    WithinFirst,
    Filter,
    Frequency,
    Transpose,
    /// Rows, then columns, from the first two arguments: positive integer
    /// constants, 1 when the call stops before them.
    Sized,
    /// `MUNIT(n)`: `n` by `n`.
    Square,
    Take,
    Drop,
    Expand,
    ChooseColumns,
    ChooseRows,
    HorizontalStack,
    VerticalStack,
    ToColumn,
    ToRow,
    WrapRows,
    WrapColumns,
    MatrixProduct,
    Row,
    Column,
    If,
    Choose,
    Let,
}

/// The rule of a built-in function, by its displayed name; `None` for a
/// function the table does not list.
fn rule(name: &str) -> Option<Rule> {
    Some(match name {
        "AND" | "AVERAGE" | "AVERAGEA" | "COLUMNS" | "CONCAT" | "COUNT" | "COUNTA"
        | "COUNTBLANK" | "FALSE" | "MAX" | "MAXA" | "MEDIAN" | "MIN" | "MINA" | "NA" | "NOW"
        | "OR" | "PI" | "PRODUCT" | "RAND" | "ROWS" | "SINGLE" | "STDEV" | "STDEV.P"
        | "STDEV.S" | "STDEVA" | "STDEVP" | "SUM" | "SUMPRODUCT" | "SUMSQ" | "TEXTJOIN"
        | "TODAY" | "TRUE" | "VAR" | "VAR.P" | "VAR.S" | "VARA" | "VARP" | "XOR" => Rule::One,
        // Every argument a value. Not the Analysis ToolPak functions
        // (`EDATE`, `EOMONTH`, `ISEVEN`, `WEEKNUM`...): Excel gives #VALUE!
        // for a range there.
        "ABS" | "ACOS" | "ACOSH" | "ASIN" | "ASINH" | "ATAN" | "ATAN2" | "ATANH" | "CEILING"
        | "CEILING.MATH" | "CHAR" | "CLEAN" | "CODE" | "CONCATENATE" | "COS" | "COSH" | "DATE"
        | "DATEVALUE" | "DAY" | "DAYS" | "DEGREES" | "EVEN" | "EXACT" | "EXP" | "FACT" | "FIND"
        | "FIXED" | "FLOOR" | "FLOOR.MATH" | "HOUR" | "IFERROR" | "IFNA" | "INT" | "ISBLANK"
        | "ISERR" | "ISERROR" | "ISLOGICAL" | "ISNA" | "ISNONTEXT" | "ISNUMBER" | "ISOWEEKNUM"
        | "ISTEXT" | "LEFT" | "LEN" | "LN" | "LOG" | "LOG10" | "LOWER" | "MID" | "MINUTE"
        | "MOD" | "MONTH" | "NOT" | "ODD" | "POWER" | "PROPER" | "RADIANS" | "REPLACE" | "REPT"
        | "RIGHT" | "ROUND" | "ROUNDDOWN" | "ROUNDUP" | "SEARCH" | "SECOND" | "SIGN" | "SIN"
        | "SINH" | "SQRT" | "SUBSTITUTE" | "TAN" | "TANH" | "TEXT" | "TIME" | "TIMEVALUE"
        | "TRIM" | "TRUNC" | "UNICHAR" | "UNICODE" | "UPPER" | "VALUE" | "WEEKDAY" | "YEAR" => {
            Rule::Each("|v")
        }
        // A range taken whole, the values it is searched for applied to each
        // element.
        "COUNTIF" | "LARGE" | "SMALL" => Rule::Each("av"),
        "AVERAGEIF" | "SUMIF" => Rule::Each("ava"),
        "COUNTIFS" => Rule::Each("|av"),
        "AVERAGEIFS" | "MAXIFS" | "MINIFS" | "SUMIFS" => Rule::Each("a|av"),
        "MATCH" => Rule::Each("vav"),
        "HLOOKUP" | "VLOOKUP" | "XMATCH" => Rule::Each("vavv"),
        "SORT" | "SORTBY" => Rule::First,
        "UNIQUE" => Rule::WithinFirst,
        "FILTER" => Rule::Filter,
        "FREQUENCY" => Rule::Frequency,
        "TRANSPOSE" => Rule::Transpose,
        "MAKEARRAY" | "RANDARRAY" | "SEQUENCE" => Rule::Sized,
        "MUNIT" => Rule::Square,
        "TAKE" => Rule::Take,
        "DROP" => Rule::Drop,
        "EXPAND" => Rule::Expand,
        "CHOOSECOLS" => Rule::ChooseColumns,
        "CHOOSEROWS" => Rule::ChooseRows,
        "HSTACK" => Rule::HorizontalStack,
        "VSTACK" => Rule::VerticalStack,
        "TOCOL" => Rule::ToColumn,
        "TOROW" => Rule::ToRow,
        "WRAPROWS" => Rule::WrapRows,
        "WRAPCOLS" => Rule::WrapColumns,
        "MMULT" => Rule::MatrixProduct,
        "ROW" => Rule::Row,
        "COLUMN" => Rule::Column,
        "IF" => Rule::If,
        "CHOOSE" => Rule::Choose,
        "LET" => Rule::Let,
        _ => return None,
    })
}

/// A count or an index argument.
#[derive(Clone, Copy)]
enum Count {
    /// Left out: all of the array.
    All,
    Constant(i64),
    /// One value the data give.
    Data,
}

/// The extent of an operation applied to each element of `a` and `b`: as
/// large as the larger in each dimension. Exact when each dimension is the
/// same or 1 in one of them; Excel fills the rest of a larger result with
/// #N/A, which is not relied on.
fn broadcast(a: Extent, b: Extent) -> Extent {
    let fits = |x: u32, y: u32| x == y || x == 1 || y == 1;
    let largest = |a: Shape, b: Shape| Shape {
        rows: a.rows.max(b.rows),
        columns: a.columns.max(b.columns),
    };
    Extent::new(
        largest(a.shape, b.shape),
        a.exact
            && b.exact
            && fits(a.shape.rows, b.shape.rows)
            && fits(a.shape.columns, b.shape.columns),
    )
    .with_data(a.data.zip(b.data).map(|(a, b)| largest(a, b)))
}

/// The rows or columns between two parts of a reference, both included.
fn span(a: &Part, b: &Part) -> u64 {
    u64::from(a.index.abs_diff(b.index)) + 1
}

/// The extent of a reference token: a cell, an area, whole columns or rows,
/// these as far as their sheet's used range (`used`) and one more for the
/// blank cells after it; `None` for a 3D reference.
fn reference_extent(token: &str, used: &UsedLookup<'_>) -> Option<Extent> {
    let start = target_start(token);
    let prefix = &token[..start];
    if prefix.contains(':') {
        return None;
    }
    // The used range of the sheet whole columns or rows are on: `None` for
    // another workbook's.
    let reach = || match prefix.strip_suffix('!') {
        None => used(None),
        Some(sheet) => used(Some(&sheet_of(sheet)?)),
    };
    let whole = |length: u64, across: u64, limit: u32, rows: bool| -> Option<Extent> {
        let full = match rows {
            true => Shape::new(u64::from(limit), across)?,
            false => Shape::new(across, u64::from(limit))?,
        };
        let data = reach().and_then(|(last_row, last_column)| {
            let reached = u64::from(if rows { last_row } else { last_column }) + 1;
            let length = reached.min(length);
            match rows {
                true => Shape::new(length, across),
                false => Shape::new(across, length),
            }
        });
        Some(Extent::exact(full).with_data(data))
    };
    match target(token)? {
        Target::Cell { .. } | Target::Deleted => Some(Extent::exact(Shape::ONE)),
        Target::Area { first, last } => {
            Shape::new(span(&first.1, &last.1), span(&first.0, &last.0)).map(Extent::exact)
        }
        Target::Columns(first, last) => {
            whole(u64::from(MAX_ROWS), span(&first, &last), MAX_ROWS, true)
        }
        Target::Rows(first, last) => whole(
            u64::from(MAX_COLUMNS),
            span(&first, &last),
            MAX_COLUMNS,
            false,
        ),
    }
}

/// The sheet a name's prefix names, unquoted: `None` without one or for
/// another workbook's (`[1]!Rate`).
fn sheet_of(prefix: &str) -> Option<String> {
    if prefix.is_empty() || prefix.starts_with('[') {
        return None;
    }
    Some(match prefix.strip_prefix('\'') {
        Some(quoted) => quoted.strip_suffix('\'')?.replace("''", "'"),
        None => prefix.to_owned(),
    })
}

/// A dimension of `TAKE`, `DROP` or `EXPAND` of an array `size` long in
/// it, exactly or at most: the length it gives, and whether exactly (a
/// length of 1 is one, exactly).
fn counted(rule: Rule, count: Count, size: u64, exact: bool) -> Option<(u64, bool)> {
    let (length, exact) = match (rule, count) {
        (_, Count::All) => (size, exact),
        (_, Count::Constant(0)) => return None,
        (Rule::Take, Count::Constant(n)) => (n.unsigned_abs().min(size), exact),
        (Rule::Drop, Count::Constant(n)) => {
            let left = size
                .checked_sub(n.unsigned_abs())
                .filter(|&left| left > 0)?;
            (left, exact)
        }
        // EXPAND pads to `n`: an array longer than `n` is an error.
        (_, Count::Constant(n)) => {
            let n = u64::try_from(n).ok()?;
            match n >= size {
                true => (n, true),
                false if exact => return None,
                false => (n, false),
            }
        }
        // TAKE and DROP give at most what they take from; EXPAND as much
        // as the data say.
        (Rule::Take | Rule::Drop, Count::Data) => (size, false),
        (_, Count::Data) => return None,
    };
    Some((length, exact || length == 1))
}

struct Inference<'p, 'n> {
    parsed: &'p Parsed<'p>,
    lookups: &'n Lookups<'n>,
    /// How many defined names deep this formula is.
    depth: usize,
    within: &'p Within,
    shared: &'p mut Shared,
    /// The LET names in force, by the key they compare by, with the extent
    /// of their value.
    scope: Vec<(String, Option<Extent>)>,
}

impl Inference<'_, '_> {
    fn extent(&mut self, node: &Node) -> Option<Extent> {
        self.shared.nodes += 1;
        if self.shared.nodes > MAX_NODES {
            return None;
        }
        match node {
            Node::Leaf(index) => self.leaf(*index),
            Node::Prefix { op, operand } => match self.parsed.token(*op) {
                "@" => Some(Extent::exact(Shape::ONE)),
                _ => self.extent(operand),
            },
            // `x%`; `x#` is the spill of another cell.
            Node::Postfix { operand, op } => match self.parsed.token(*op) {
                "%" => self.extent(operand),
                _ => None,
            },
            Node::Operation { first, rest } => {
                let mut extent = self.extent(first)?;
                for (op, operand) in rest {
                    if self.parsed.kind(*op) != TokenKind::Operator
                        || !ELEMENT_OPERATORS.contains(&self.parsed.token(*op))
                    {
                        return None;
                    }
                    extent = broadcast(extent, self.extent(operand)?);
                }
                Some(extent)
            }
            Node::Paren { inner, .. } => self.extent(inner),
            Node::Array { open, close } => self.array(*open, *close),
            Node::Call { name, args, .. } => self.call(*name, args),
            Node::Invoke { .. } => None,
        }
    }

    fn leaf(&mut self, index: usize) -> Option<Extent> {
        let token = self.parsed.token(index);
        match self.parsed.kind(index) {
            TokenKind::Number | TokenKind::Text | TokenKind::Boolean | TokenKind::Error => {
                Some(Extent::exact(Shape::ONE))
            }
            TokenKind::Reference => reference_extent(token, self.lookups.used),
            TokenKind::Name => self.name(token),
            _ => None,
        }
    }

    /// A LET name in force, else a defined name; `None` for a LAMBDA
    /// parameter.
    fn name(&mut self, token: &str) -> Option<Extent> {
        let start = target_start(token);
        if start == 0 {
            let (prefix, _, key) = parameter(token);
            if let Some((_, extent)) = self.scope.iter().rev().find(|(name, _)| *name == key) {
                return *extent;
            }
            if !prefix.is_empty() {
                return None;
            }
        }
        if self.depth >= MAX_NAME_DEPTH {
            return None;
        }
        let sheet = match start {
            0 => None,
            _ => Some(sheet_of(&token[..start - 1])?),
        };
        let name = &token[start..];
        let (scope, key) = match (&sheet, self.within) {
            (Some(sheet), _) => (NameScope::Sheet(sheet), format!("s:{sheet}")),
            (None, Within::Formula) => (NameScope::Formula, "f".to_owned()),
            (None, Within::Sheet(owner)) => (NameScope::WithinSheet(owner), format!("w:{owner}")),
            (None, Within::Workbook) => (NameScope::Workbook, "b".to_owned()),
        };
        let key = format!("{}!{}", key.to_lowercase(), name.to_lowercase());
        if let Some(known) = self.shared.names.get(&key) {
            return *known;
        }
        self.shared.names.insert(key.clone(), None);
        let defined = (self.lookups.names)(scope, name);
        let extent = defined.and_then(|defined| {
            let within = match defined.sheet {
                Some(owner) => Within::Sheet(owner),
                None => Within::Workbook,
            };
            extent_of(
                &defined.formula,
                self.lookups,
                self.depth + 1,
                &within,
                self.shared,
            )
        });
        self.shared.names.insert(key, extent);
        extent
    }

    /// An array constant: `,` between columns, `;` between rows.
    fn array(&self, open: usize, close: usize) -> Option<Extent> {
        let mut rows = vec![0u64];
        for index in open + 1..close {
            match self.parsed.kind(index) {
                TokenKind::Semicolon => rows.push(0),
                TokenKind::Number | TokenKind::Text | TokenKind::Boolean | TokenKind::Error => {
                    *rows.last_mut().expect("a row") += 1;
                }
                _ => {}
            }
        }
        if rows.iter().any(|&length| length != rows[0]) {
            return None;
        }
        Extent::of(rows.len() as u64, rows[0], true)
    }

    /// The extent of argument `index`, `None` when it is left out.
    fn argument(&mut self, args: &[Option<Node>], index: usize) -> Option<Extent> {
        self.extent(args.get(index)?.as_ref()?)
    }

    /// The extent of a value argument: one value when it is left out.
    fn value(&mut self, args: &[Option<Node>], index: usize) -> Option<Extent> {
        match args.get(index) {
            Some(Some(node)) => self.extent(node),
            _ => Some(Extent::exact(Shape::ONE)),
        }
    }

    /// The integer a constant holds: `5`, `-2`, or `ROWS` or `COLUMNS` of an
    /// exact extent; `None` for anything else (a reference, a calculation,
    /// a fraction).
    fn integer(&mut self, node: &Node) -> Option<i64> {
        let (negative, leaf) = match node {
            Node::Leaf(leaf) => (false, *leaf),
            Node::Prefix { op, operand } => match (self.parsed.token(*op), &**operand) {
                ("-", Node::Leaf(leaf)) => (true, *leaf),
                ("+", Node::Leaf(leaf)) => (false, *leaf),
                _ => return None,
            },
            Node::Call { name, args, .. } => {
                let measure = function(self.parsed.token(*name))?.name;
                let extent = match (measure, &args[..]) {
                    ("ROWS" | "COLUMNS", [Some(arg)]) => self.extent(arg)?,
                    _ => return None,
                };
                return extent.exact.then_some(match measure {
                    "ROWS" => i64::from(extent.shape.rows),
                    _ => i64::from(extent.shape.columns),
                });
            }
            _ => return None,
        };
        if self.parsed.kind(leaf) != TokenKind::Number {
            return None;
        }
        let value: f64 = self.parsed.token(leaf).parse().ok()?;
        if value.fract() != 0.0 || value.abs() > 1e15 {
            return None;
        }
        let value = value as i64;
        Some(if negative { -value } else { value })
    }

    /// A size argument: a positive integer constant, 1 when the call stops
    /// before it.
    fn size(&mut self, args: &[Option<Node>], index: usize) -> Option<u64> {
        match args.get(index) {
            None => Some(1),
            Some(node) => u64::try_from(self.integer(node.as_ref()?)?)
                .ok()
                .filter(|&size| size > 0),
        }
    }

    /// A count or index argument: left out (missing or empty), a constant,
    /// or one value the data give.
    fn count(&mut self, args: &[Option<Node>], index: usize) -> Option<Count> {
        let Some(Some(node)) = args.get(index) else {
            return Some(Count::All);
        };
        if let Some(constant) = self.integer(node) {
            return Some(Count::Constant(constant));
        }
        (self.extent(node)?.shape == Shape::ONE).then_some(Count::Data)
    }

    fn call(&mut self, name: usize, args: &[Option<Node>]) -> Option<Extent> {
        let rule = rule(function(self.parsed.token(name))?.name)?;
        match rule {
            Rule::One => Some(Extent::exact(Shape::ONE)),
            Rule::Each(pattern) => {
                let (head, repeated) = pattern.split_once('|').unwrap_or((pattern, ""));
                let (head, repeated) = (head.as_bytes(), repeated.as_bytes());
                let mut extent = Extent::exact(Shape::ONE);
                for index in 0..args.len() {
                    let letter = match head.get(index) {
                        Some(letter) => *letter,
                        None if !repeated.is_empty() => {
                            repeated[(index - head.len()) % repeated.len()]
                        }
                        None => return None,
                    };
                    if letter == b'v' {
                        extent = broadcast(extent, self.value(args, index)?);
                    }
                }
                Some(extent)
            }
            Rule::First => self.argument(args, 0),
            // What the data fill of what it reads: its blank cells give one
            // value more.
            Rule::WithinFirst => {
                let read = self.argument(args, 0)?.data?;
                Some(Extent::new(read, false))
            }
            // What it filters, or `if_empty` when nothing is left. Each blank
            // row of whole columns may pass on its own, so these give none.
            Rule::Filter => {
                let filtered = self.argument(args, 0)?;
                if filtered.data != Some(filtered.shape) {
                    return None;
                }
                let empty = self.value(args, 2)?;
                Some(Extent::new(broadcast(filtered, empty).shape, false))
            }
            Rule::Frequency => {
                let bins = self.argument(args, 1)?.data?;
                Extent::of(u64::from(bins.rows) * u64::from(bins.columns) + 1, 1, false)
            }
            Rule::Transpose => {
                let extent = self.argument(args, 0)?;
                let swap = |shape: Shape| Shape {
                    rows: shape.columns,
                    columns: shape.rows,
                };
                Extent::of(extent.columns(), extent.rows(), extent.exact)
                    .map(|swapped| swapped.with_data(extent.data.map(swap)))
            }
            Rule::Sized => Extent::of(self.size(args, 0)?, self.size(args, 1)?, true),
            Rule::Square => {
                let size = self.size(args, 0)?;
                Extent::of(size, size, true)
            }
            Rule::Take | Rule::Drop | Rule::Expand => {
                let array = self.argument(args, 0)?;
                let rows = counted(rule, self.count(args, 1)?, array.rows(), array.exact)?;
                let columns = counted(rule, self.count(args, 2)?, array.columns(), array.exact)?;
                Extent::of(rows.0, columns.0, rows.1 && columns.1)
            }
            Rule::ChooseColumns | Rule::ChooseRows => {
                let array = self.argument(args, 0)?;
                let size = match rule {
                    Rule::ChooseColumns => array.columns(),
                    _ => array.rows(),
                };
                let mut exact = array.exact;
                for index in 1..args.len() {
                    match self.count(args, index)? {
                        Count::Constant(n) if n != 0 && n.unsigned_abs() <= size => {}
                        // An index the data give may be out of the array.
                        Count::Data => exact = false,
                        _ => return None,
                    }
                }
                let chosen = (args.len() - 1) as u64;
                match rule {
                    Rule::ChooseColumns => Extent::of(array.rows(), chosen, exact),
                    _ => Extent::of(chosen, array.columns(), exact),
                }
            }
            Rule::HorizontalStack | Rule::VerticalStack => {
                // The arrays laid end to end, as wide as the widest.
                let horizontal = rule == Rule::HorizontalStack;
                let (mut length, mut width, mut exact) = (0u64, 0u64, true);
                for index in 0..args.len() {
                    let extent = self.argument(args, index)?;
                    let (along, across) = match horizontal {
                        true => (extent.columns(), extent.rows()),
                        false => (extent.rows(), extent.columns()),
                    };
                    length += along;
                    width = width.max(across);
                    exact &= extent.exact;
                }
                match horizontal {
                    true => Extent::of(width, length, exact),
                    false => Extent::of(length, width, exact),
                }
            }
            Rule::ToColumn | Rule::ToRow => {
                let array = self.argument(args, 0)?;
                // What it skips (blanks, errors) the data decide.
                let skips = match self.count(args, 1)? {
                    Count::All | Count::Constant(0) => false,
                    Count::Constant(_) | Count::Data => true,
                };
                let cells = array.rows() * array.columns();
                let exact = array.exact && !skips;
                match rule {
                    Rule::ToColumn => Extent::of(cells, 1, exact),
                    _ => Extent::of(1, cells, exact),
                }
            }
            Rule::WrapRows | Rule::WrapColumns => {
                let vector = self.argument(args, 0)?;
                let length = match vector.shape {
                    Shape { rows: 1, columns } => u64::from(columns),
                    Shape { rows, columns: 1 } => u64::from(rows),
                    _ => return None,
                };
                let wrap = match args.get(1) {
                    Some(_) => self.size(args, 1)?,
                    None => return None,
                };
                // As many lines as it takes, each as long as the wrap or the
                // vector, whichever is shorter (checked in Excel 16.115).
                let lines = length.div_ceil(wrap);
                let line = wrap.min(length);
                match rule {
                    Rule::WrapRows => Extent::of(lines, line, vector.exact),
                    _ => Extent::of(line, lines, vector.exact),
                }
            }
            Rule::MatrixProduct => {
                let (a, b) = (self.argument(args, 0)?, self.argument(args, 1)?);
                if a.exact && b.exact && a.shape.columns != b.shape.rows {
                    return None;
                }
                Extent::of(a.rows(), b.columns(), a.exact && b.exact)
            }
            Rule::Row | Rule::Column => match args.first() {
                None => Some(Extent::exact(Shape::ONE)),
                Some(_) => {
                    let extent = self.argument(args, 0)?;
                    match rule {
                        Rule::Row => Extent::of(extent.rows(), 1, extent.exact),
                        _ => Extent::of(1, extent.columns(), extent.exact),
                    }
                }
            },
            Rule::If | Rule::Choose => {
                let selector = self.argument(args, 0)?;
                // IF's choices are its next two arguments, FALSE when left
                // out; CHOOSE's all the others.
                let count = match rule {
                    Rule::If => 3,
                    _ => args.len(),
                };
                let mut choices = Vec::new();
                for index in 1..count {
                    choices.push(self.value(args, index)?);
                }
                if selector.shape == Shape::ONE {
                    // One of the choices: exact when they all have its shape.
                    let first = *choices.first()?;
                    let largest = choices.iter().fold(first, |a, &b| broadcast(a, b));
                    let same = choices.iter().all(|&choice| choice.same(first));
                    Some(Extent::new(largest.shape, same && first.exact))
                } else {
                    Some(choices.into_iter().fold(selector, broadcast))
                }
            }
            Rule::Let => self.let_call(args),
        }
    }

    /// `LET(name, value, ..., calculation)`: each name in force from the
    /// value after its own to the calculation.
    fn let_call(&mut self, args: &[Option<Node>]) -> Option<Extent> {
        let (calculation, pairs) = args.split_last()?;
        if pairs.len() % 2 != 0 {
            return None;
        }
        let base = self.scope.len();
        let mut declared = true;
        for pair in pairs.chunks(2) {
            let [Some(Node::Leaf(leaf)), value] = pair else {
                declared = false;
                break;
            };
            let key = parameter(self.parsed.token(*leaf)).2;
            let value = value.as_ref().and_then(|value| self.extent(value));
            self.scope.push((key, value));
        }
        let result = match calculation {
            Some(calculation) if declared => self.extent(calculation),
            _ => None,
        };
        self.scope.truncate(base);
        result
    }
}

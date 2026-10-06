use crate::InkResult;
use crate::errors::InkError;
use crate::record::Value;

use super::parser::ExprArena;
use super::tokens::TokenKind;
use std::rc::Rc;

/// What a parsed statement is made of.
///
/// Every statement type has a struct below, and [`Ast`] is the one enum that
/// holds them all, which is how the rest of the engine takes a statement it has
/// already read without caring which kind it is. Expressions do not live in
/// these structs; they are held by index in the arena that comes along with
/// them, and a name that has not been checked against the schema yet is still a
/// name here.
#[derive(Debug, Clone)]
pub struct CreateTableStmt {
    pub(crate) query: Rc<str>,
    pub(crate) name: String,
    pub(crate) columns: Vec<Column>,
    pub(crate) tbl_constraints: Vec<usize>,
    pub(crate) arena: ExprArena,
}

#[derive(Debug)]
pub struct CreateIndexStmt {
    pub(crate) query: Rc<str>,
    pub(crate) unique: bool,
    pub(crate) name: String,
    pub(crate) table: String,
    pub(crate) columns: Vec<String>,
}
#[derive(Debug)]
pub struct DropTableStmt {
    pub(crate) tbl_name: String,
}
#[derive(Debug)]
pub struct DropIndexStmt {
    pub(crate) index_name: String,
}
/// One column of a table definition.
#[derive(Debug, Clone)]
pub(crate) struct Column {
    pub(crate) name: String,
    pub(crate) affinity: Affinity,
    pub(crate) constraints: Option<Box<[Constraint]>>,
    pub(crate) default: Option<DefaultValue>,
}
impl Column {
    /// Whether this column was declared with the given [`constraint`](Constraint).
    pub(crate) fn has_constraint(&self, constraint: Constraint) -> bool {
        self.constraints
            .as_ref()
            .is_some_and(|csts| csts.contains(&constraint))
    }
    /// Whether the column was declared `UNIQUE`
    /// ( e.g. CREATE UNIQUE index x on t(c) )
    pub(crate) fn is_unique(&self) -> bool {
        self.has_constraint(Constraint::Unique)
    }
    /// Whether the column was declared `PRIMARY KEY`.
    pub(crate) fn has_primary_key(&self) -> bool {
        self.has_constraint(Constraint::PrimaryKey)
    }
}
/// What a column falls back to when an insert does not mention it.
/// It can only be defined by the user at the time of table creation.
///
/// The two variants are the same default at two different moments:
/// `Node` while it is still an expression (e.g. DEFAULT a+b/c*d), the node represent
///  the root of the expression inside the (`arena`), waiting
///  to be evaluated.
/// `Val` once it has been worked out and can be stored directly.
#[derive(Debug, Clone)]
pub(crate) enum DefaultValue {
    Node(usize),
    Val(Value<'static>),
}

/// One expression, stored in the arena.
///
/// Children are indices into the same arena rather than nested expressions, so
/// this type stays small and the parts of an expression can be compared and
/// rewritten without copying.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Expr {
    Number(i64),
    Float(f64),
    StringLitteral(String),
    Blob(Vec<u8>),
    Bool(bool),
    ColumnRef(usize),
    Identifier(String),
    Add(usize, usize),
    Substract(usize, usize),
    Null,
    Devide(usize, usize),
    Multiply(usize, usize),
    Neg(usize),
    Not(usize),
    Star,

    Count {
        arg: Option<usize>,
    },

    BinaryOp {
        left: usize,
        op: BinaryOperator,
        right: usize,
    },

    And {
        left: usize,
        right: usize,
    },

    Or {
        left: usize,
        right: usize,
    },
}
impl Expr {
    /// The same kind of expression as `expr`, with its two children pointing at
    /// `l` and `r` instead. Only the kinds that have exactly two children are
    /// accepted, so the select rebuild uses it after checking the kind itself.
    /// This used to correct the offset of the expressions inside the
    /// arena.
    ///
    /// # Panics
    /// When handed an expression kind that has no left and right to remap, which
    /// would mean the caller expected children that are not there.
    pub(crate) fn remap_l_r(expr: &Expr, l: usize, r: usize) -> Expr {
        match expr {
            Expr::Add(_, _) => Expr::Add(l, r),
            Expr::Substract(_, _) => Expr::Substract(l, r),
            Expr::Multiply(_, _) => Expr::Multiply(l, r),
            Expr::Devide(_, _) => Expr::Devide(l, r),
            Expr::And { left: _, right: _ } => Expr::And { left: l, right: r },
            Expr::Or { left: _, right: _ } => Expr::Or { left: l, right: r },
            Expr::BinaryOp { op, .. } => Expr::BinaryOp {
                left: l,
                op: *op,
                right: r,
            },
            // With the correct behaviour, this should never be reached
            _ => unreachable!("Reached unmapped Expression"),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Copy)]
pub(crate) enum BinaryOperator {
    Eq,
    NotEq,
    Ge,
    Le,
    Gt,
    Lt,
    Is,
    IsNot,
}

/// A `SELECT`, after parsing but before anything is checked against a table.
#[derive(Debug, Clone)]
pub struct SelectStmt {
    pub(crate) table_name: Option<String>,
    pub(crate) arena: ExprArena,
    pub(crate) columns: Vec<usize>,
    pub(crate) where_clause: Option<usize>,
    pub(crate) limit: Option<usize>,
    pub(crate) orderby: Option<OrderBy>, /*Order by is limited to one expression for now*/
}

/// How the result should be ordered: the expression to sort on, and whether it
/// should come out descending.
#[derive(Clone, Copy, Debug)]
pub(crate) struct OrderBy {
    pub(crate) index: usize,
    pub(crate) desc: bool,
}

impl OrderBy {
    pub(crate) fn new(index: usize, desc: bool) -> Self {
        Self { index, desc }
    }
}
/// An `INSERT`. The column list may be empty, which means the values line up
/// with the columns in the order the table declares them.
#[derive(Debug)]
pub struct InsertStmt {
    pub(crate) table_name: String,
    pub(crate) columns: Vec<String>,
    pub(crate) values: Vec<Vec<usize>>,
    pub(crate) arena: ExprArena,
}

/// A `DELETE`, with or without a `WHERE` clause. The arena is only there when
/// there is a condition to hold.
#[derive(Debug)]
pub struct DeleteStmt {
    pub(crate) table_name: String,
    pub(crate) arena: Option<ExprArena>,
    pub(crate) where_clause: Option<usize>,
}

/// `EXPLAIN`, which wraps whatever statement follows it.
#[derive(Debug)]
pub struct ExplainStmt {
    pub(crate) query: Box<Ast>,
}

/// How a value is meant to be stored and compared in a column.
///
/// The declaration `TEXT` or `INT` is a preference rather than a promise, so a
/// column with integer affinity will still hold text if it is given text.
/// This only decides which conversion is attempted first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Affinity {
    Text,
    Float,
    Int,
    Blob,
}

impl<'a> TryFrom<&Value<'a>> for Affinity {
    type Error = InkError;
    fn try_from(value: &Value<'a>) -> Result<Self, Self::Error> {
        match value {
            Value::Integer(_) => Ok(Affinity::Int),
            Value::Float(_) => Ok(Affinity::Float),
            Value::Text(_) => Ok(Affinity::Text),
            Value::Blob(_) => Ok(Affinity::Blob),
            _ => Err(InkError::runtime("Null cannot be used as column affinity")),
        }
    }
}
impl std::fmt::Display for Affinity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Blob => write!(f, "Blob"),
            Self::Text => write!(f, "Text"),
            Self::Int => write!(f, "Int"),
            Self::Float => write!(f, "Float"),
        }
    }
}

impl Affinity {
    /// Work out the affinity from the type written in a column definition.
    ///
    /// The check is on the letters the name contains, not on a list of accepted
    /// spellings, so `VARCHAR(12)` counts as TEXT and `DOUBLE` counts as a FLOAT.
    ///
    /// # Errors
    /// An error naming the type when none of the rules match it.
    pub(crate) fn from_type_name(name: &str) -> InkResult<Self> {
        let upper = name.to_uppercase();
        if upper.contains("INT") {
            Ok(Self::Int)
        } else if upper.contains("CHAR") || upper.contains("CLOB") || upper.contains("TEXT") {
            Ok(Self::Text)
        } else if upper.contains("REAL") || upper.contains("FLOA") || upper.contains("DOUB") {
            Ok(Self::Float)
        } else {
            Err(InkError::runtime(format!("No affinity matches: {}", name)))
        }
    }
}

/// The constraints that can be written on a single column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Constraint {
    PrimaryKey,
    NotNull,
    Unique,
}
#[derive(Debug)]
pub struct TruncateTableStmt {
    pub(crate) table_name: String,
}

/// An `UPDATE`. Each assignment pairs the expression naming the column with the
/// expression that produces its new value.
#[derive(Debug)]
pub struct UpdateStmt {
    pub(crate) table_name: String,
    pub(crate) columns: Vec<(usize, usize)>,
    pub(crate) where_clause: Option<usize>,
    pub(crate) arena: ExprArena,
}

impl UpdateStmt {
    /// Build an update from its parts.
    pub fn new(
        table_name: String,
        columns: Vec<(usize, usize)>,
        where_clause: Option<usize>,
        arena: ExprArena,
    ) -> Self {
        Self {
            table_name,
            columns,
            where_clause,
            arena,
        }
    }
}

/// One parsed statement, whichever kind it is.
#[derive(Debug)]
pub enum Ast {
    BeginTransaction,
    CommitTransaction,
    CreateIndexAst(CreateIndexStmt),
    CreateTableAst(CreateTableStmt),
    DeleteStmtAst(DeleteStmt),
    DropIndexAst(DropIndexStmt),
    DropTblAst(DropTableStmt),
    ExplainStmtAst(ExplainStmt),
    InsertStmtAst(InsertStmt),
    RollbackTransaction,
    SelectStmtAst(SelectStmt),
    TruncateTableAst(TruncateTableStmt),
    UpdateStmtAst(UpdateStmt),
}
impl From<TokenKind> for Affinity {
    /// The affinity a type keyword stands for.
    ///
    /// # Panics
    /// When handed a keyword that is not a type, which would mean the caller did
    /// not check what it was looking at first.
    fn from(value: TokenKind) -> Self {
        match value {
            TokenKind::Integer | TokenKind::Bool => Self::Int,
            TokenKind::Text => Self::Text,
            TokenKind::Float => Self::Float,
            TokenKind::Blob => Self::Blob,
            // The value is checked before this is called.
            _ => unreachable!(),
        }
    }
}

use std::fmt;

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Number(n) => write!(f, "{n}"),
            Expr::Float(n) => write!(f, "{n}"),
            Expr::Null => write!(f, "Null"),
            Expr::StringLitteral(s) => write!(f, "'{s}'"),
            Expr::Blob(bytes) => {
                write!(
                    f,
                    "X'{}'",
                    bytes.iter().map(|b| format!("{b:02X}")).collect::<String>()
                )
            }
            Expr::Bool(b) => write!(f, "{b}"),
            Expr::ColumnRef(idx) => write!(f, "column[{idx}]"),
            Expr::Identifier(s) => write!(f, "{s}"),

            Expr::Add(left, right) => write!(f, "({left} + {right})"),
            Expr::Substract(left, right) => write!(f, "({left} - {right})"),
            Expr::Devide(left, right) => write!(f, "({left} / {right})"),
            Expr::Multiply(left, right) => write!(f, "({left} * {right})"),

            Expr::Neg(expr) => write!(f, "-{expr}"),
            Expr::Not(expr) => write!(f, "NOT {expr}"),
            Expr::Star => write!(f, "*"),
            Expr::Count { arg: Some(arg) } => write!(f, "count({arg})"),
            Expr::Count { arg: None } => write!(f, "count(*)"),

            Expr::BinaryOp { left, op, right } => {
                write!(f, "({left} {op} {right})")
            }

            Expr::And { left, right } => {
                write!(f, "({left} AND {right})")
            }

            Expr::Or { left, right } => {
                write!(f, "({left} OR {right})")
            }
        }
    }
}

impl fmt::Display for BinaryOperator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let op = match self {
            BinaryOperator::Eq => "=",
            BinaryOperator::NotEq => "!=",
            BinaryOperator::Ge => ">=",
            BinaryOperator::Le => "<=",
            BinaryOperator::Gt => ">",
            BinaryOperator::Lt => "<",
            BinaryOperator::Is => "IS",
            BinaryOperator::IsNot => "IS NOT",
        };

        write!(f, "{op}")
    }
}

//! The predicates that can be pushed down: an AND of simple terms.

use crate::stats::Value;

/// A comparison operator. `x <op> literal`, the column on the left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmpOp {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

/// `column <op> literal`, `column IS NULL` or `column IS NOT NULL`.
///
/// There is no NULL literal: `x = NULL` is never true, and the binder folds it to `false` before pruning is asked.
#[derive(Clone, Debug, PartialEq)]
pub enum Term {
    Cmp {
        column: String,
        op: CmpOp,
        literal: Value,
    },
    IsNull {
        column: String,
    },
    IsNotNull {
        column: String,
    },
}

impl Term {
    pub fn cmp(column: &str, op: CmpOp, literal: Value) -> Term {
        Term::Cmp {
            column: column.to_string(),
            op,
            literal,
        }
    }

    pub fn is_null(column: &str) -> Term {
        Term::IsNull {
            column: column.to_string(),
        }
    }

    pub fn is_not_null(column: &str) -> Term {
        Term::IsNotNull {
            column: column.to_string(),
        }
    }
}

/// A conjunction (AND) of terms. A row matches when EVERY term is true for it. The empty conjunction is true.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Predicate {
    pub terms: Vec<Term>,
}

impl Predicate {
    pub fn new(terms: Vec<Term>) -> Predicate {
        Predicate { terms }
    }

    pub fn and(mut self, term: Term) -> Predicate {
        self.terms.push(term);
        self
    }
}

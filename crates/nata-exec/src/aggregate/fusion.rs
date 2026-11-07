//! Eager aggregate expression DAG. Conditional roots stay in the selected-row
//! evaluator. Successful common nodes share a column; their first occurrence
//! retains its original error position and evaluation order.
use super::*;
use nata_sql::texpr::{Expr, ExprKind, Value};
use nata_sql::types::ArithOp;

#[derive(PartialEq)]
enum Code {
    Column(usize),
    Literal(Value),
    Arith(ArithOp, usize, usize),
    Cast(usize),
    Generic(usize),
}
struct Node {
    code: Code,
    // Only type and position are needed by the shared strict operations.
    meta: Expr,
}
type Arguments = (Vec<Arc<Col>>, Vec<Option<Arc<Col>>>);

pub(super) struct Program {
    nodes: Vec<Node>,
    columns: Vec<Arc<Col>>,
    keys: Vec<usize>,
    args: Vec<Option<usize>>,
    empty_text: Arc<arrow_array::StringArray>,
}
fn eager(e: &Expr) -> bool {
    match &e.kind {
        // Keep plan-owned text constants in the generic evaluator instead of
        // retaining an additional owned copy in the task instruction arena.
        ExprKind::Literal(Value::Str(_)) => false,
        ExprKind::Column(_) | ExprKind::Literal(_) => true,
        ExprKind::Arith { left, right, .. } => eager(left) && eager(right),
        ExprKind::Cast { expr, .. } => eager(expr),
        _ => false,
    }
}
// Compare typed structure, not source positions. Only complete conditional
// roots share a result: their internal branches retain independent selections.
fn same_expression(l: &Expr, r: &Expr) -> bool {
    if l.ty != r.ty {
        return false;
    }
    let same = match (&l.kind, &r.kind) {
        (ExprKind::Column(a), ExprKind::Column(b)) => a == b,
        (ExprKind::Literal(a), ExprKind::Literal(b)) => a == b,
        (ExprKind::Arith { op: a, .. }, ExprKind::Arith { op: b, .. }) => a == b,
        (ExprKind::Cmp { op: a, .. }, ExprKind::Cmp { op: b, .. }) => a == b,
        (ExprKind::IsNull { negated: a, .. }, ExprKind::IsNull { negated: b, .. })
        | (ExprKind::Between { negated: a, .. }, ExprKind::Between { negated: b, .. })
        | (ExprKind::InList { negated: a, .. }, ExprKind::InList { negated: b, .. }) => a == b,
        (
            ExprKind::Case {
                operand: a,
                whens: aw,
                ..
            },
            ExprKind::Case {
                operand: b,
                whens: bw,
                ..
            },
        ) => a.is_some() == b.is_some() && aw.len() == bw.len(),
        (ExprKind::Neg(_), ExprKind::Neg(_))
        | (ExprKind::Not(_), ExprKind::Not(_))
        | (ExprKind::And(_), ExprKind::And(_))
        | (ExprKind::Or(_), ExprKind::Or(_))
        | (ExprKind::Cast { .. }, ExprKind::Cast { .. }) => true,
        _ => false,
    };
    if !same {
        return false;
    }
    let lc = l.children();
    let rc = r.children();
    lc.len() == rc.len() && lc.into_iter().zip(rc).all(|(l, r)| same_expression(l, r))
}

impl Program {
    pub(super) fn new(a: &Aggregate) -> Self {
        let mut out = Self {
            nodes: vec![],
            columns: vec![],
            keys: vec![],
            args: vec![],
            empty_text: Arc::new(arrow_array::StringArray::from(Vec::<&str>::new())),
        };
        let roots: Vec<_> = a
            .keys
            .iter()
            .map(Some)
            .chain(a.aggregates.iter().map(|a| a.arg.as_ref()))
            .collect();
        for (i, e) in a.keys.iter().enumerate() {
            let id = out.root(e, i, &roots);
            out.keys.push(id);
        }
        for (i, agg) in a.aggregates.iter().enumerate() {
            let id = agg
                .arg
                .as_ref()
                .map(|e| out.root(e, a.keys.len() + i, &roots));
            out.args.push(id);
        }
        out
    }
    fn root(&mut self, e: &Expr, root: usize, roots: &[Option<&Expr>]) -> usize {
        if eager(e) {
            self.compile(e)
        } else if let Some(id) = self.nodes.iter().position(|n| {
            if let Code::Generic(other) = n.code {
                roots[other].is_some_and(|prior| same_expression(e, prior))
            } else {
                false
            }
        }) {
            id
        } else {
            self.push(Code::Generic(root), e)
        }
    }
    fn compile(&mut self, e: &Expr) -> usize {
        let code = match &e.kind {
            ExprKind::Column(i) => Code::Column(*i),
            ExprKind::Literal(v) => Code::Literal(v.clone()),
            ExprKind::Arith { op, left, right } => {
                let l = self.compile(left);
                let r = self.compile(right);
                Code::Arith(*op, l, r)
            }
            ExprKind::Cast { expr, .. } => Code::Cast(self.compile(expr)),
            _ => unreachable!("eligible eager tree"),
        };
        self.push(code, e)
    }
    fn push(&mut self, code: Code, e: &Expr) -> usize {
        if let Some(i) = self
            .nodes
            .iter()
            .position(|n| n.code == code && n.meta.ty == e.ty)
        {
            return i;
        }
        let id = self.nodes.len();
        self.nodes.push(Node {
            code,
            meta: Expr::literal(Value::Null, e.ty, e.pos),
        });
        self.columns.push(Arc::new(Col::empty(e.ty, 0)));
        id
    }
    pub(super) fn evaluate(
        &mut self,
        batch: &RecordBatch,
        ev: &Evaluator<'_>,
        a: &Aggregate,
        rows: &[u32],
    ) -> R<Arguments> {
        for (id, node) in self.nodes.iter().enumerate() {
            let (inputs, outputs) = self.columns.split_at_mut(id);
            let out = Arc::get_mut(&mut outputs[0])
                .ok_or_else(|| EvalError::internal("live aggregate expression buffer"))?;
            out.reset(batch.num_rows());
            match &node.code {
                Code::Column(i) => out.read_arrow(batch.column(*i))?,
                Code::Literal(v) => crate::eval::literal_into(v, out)?,
                Code::Arith(op, l, r) => {
                    crate::eval::arith_into(*op, &inputs[*l], &inputs[*r], &node.meta, rows, out)?
                }
                Code::Cast(i) => {
                    crate::cast::cast_into(&inputs[*i], node.meta.ty, rows, node.meta.pos, out)?
                }
                Code::Generic(root) => {
                    let expr = if *root < a.keys.len() {
                        &a.keys[*root]
                    } else {
                        a.aggregates[*root - a.keys.len()]
                            .arg
                            .as_ref()
                            .ok_or_else(resource)?
                    };
                    let col = ev.eval(expr, rows)?;
                    *out = std::rc::Rc::try_unwrap(col).unwrap_or_else(|c| (*c).clone());
                }
            }
        }
        Ok((
            self.keys
                .iter()
                .map(|&i| Arc::clone(&self.columns[i]))
                .collect(),
            self.args
                .iter()
                .map(|i| i.map(|i| Arc::clone(&self.columns[i])))
                .collect(),
        ))
    }
    // Do not retain previous Arrow dictionaries/expanded text while decoding
    // another batch. Numeric/key capacities remain within the admitted scratch.
    pub(super) fn release_text(&mut self) {
        for col in &mut self.columns {
            if let Some(c) = Arc::get_mut(col) {
                match &mut c.data {
                    Data::Str(StrData::Dict { values, keys }) => {
                        *values = Arc::clone(&self.empty_text);
                        keys.clear();
                    }
                    Data::Str(StrData::Owned(values)) => values.clear(),
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::Int64Array;
    use nata_sql::Pos;
    fn col() -> Expr {
        Expr::new(ExprKind::Column(0), SqlType::BigInt, Pos::START)
    }
    fn lit(v: i64) -> Expr {
        Expr::literal(Value::Int(v), SqlType::BigInt, Pos::START)
    }
    fn binary(op: ArithOp, l: Expr, r: Expr, pos: Pos) -> Expr {
        Expr::new(
            ExprKind::Arith {
                op,
                left: Box::new(l),
                right: Box::new(r),
            },
            SqlType::BigInt,
            pos,
        )
    }
    fn product() -> Expr {
        binary(ArithOp::Mul, col(), lit(2), Pos::START)
    }
    fn aggregate(exprs: Vec<Expr>) -> Aggregate {
        Aggregate {
            keys: vec![],
            aggregates: exprs
                .into_iter()
                .map(|e| AggregateExpr {
                    name: "sum".into(),
                    arg: Some(e),
                    ty: SqlType::BigInt,
                    pos: Pos::START,
                })
                .collect(),
            having: None,
        }
    }
    fn batch(values: Vec<Option<i64>>) -> RecordBatch {
        RecordBatch::try_from_iter([(
            "x",
            Arc::new(Int64Array::from(values)) as arrow_array::ArrayRef,
        )])
        .expect("batch")
    }
    #[test]
    fn shared_eager_arguments_reuse_buffers_and_reset_selection_and_nulls() {
        let a = aggregate(vec![
            product(),
            binary(ArithOp::Add, product(), col(), Pos::START),
            product(),
        ]);
        let mut program = Program::new(&a);
        assert_eq!(
            program.nodes.len(),
            4,
            "column, literal, shared product, addition"
        );
        let first = batch(vec![Some(i64::MAX), None, Some(3), Some(4)]);
        let types = [SqlType::BigInt];
        let ev = Evaluator::new(&first, &types);
        let (keys, args) = program
            .evaluate(&first, &ev, &a, &[1, 2])
            .expect("masked overflow");
        assert!(Arc::ptr_eq(
            args[0].as_ref().expect("arg"),
            args[2].as_ref().expect("arg")
        ));
        assert_eq!(Atom::get(args[0].as_ref().expect("arg"), 1), Atom::Null);
        assert_eq!(Atom::get(args[1].as_ref().expect("arg"), 2), Atom::Int(9));
        let pointers: Vec<_> = program
            .columns
            .iter()
            .map(|c| {
                if let Data::Int(v) = &c.data {
                    v.as_ptr()
                } else {
                    panic!("int")
                }
            })
            .collect();
        drop((keys, args));
        let second = batch(vec![Some(8), Some(5), None, Some(6)]);
        let ev = Evaluator::new(&second, &types);
        let (_, args) = program
            .evaluate(&second, &ev, &a, &[0, 2, 3])
            .expect("reuse");
        assert_eq!(Atom::get(args[0].as_ref().expect("arg"), 0), Atom::Int(16));
        assert_eq!(
            Atom::get(args[0].as_ref().expect("arg"), 1),
            Atom::Null,
            "old selected bit cleared"
        );
        assert_eq!(Atom::get(args[0].as_ref().expect("arg"), 2), Atom::Null);
        for (c, ptr) in program.columns.iter().zip(pointers) {
            let Data::Int(v) = &c.data else { panic!("int") };
            assert_eq!(v.as_ptr(), ptr, "capacity survives another batch");
        }
    }
    #[test]
    fn decimal64_and_decimal128_share_checked_arithmetic_and_reused_output() {
        use arrow_array::{ArrayRef, Decimal64Array, Decimal128Array};
        let ty = SqlType::Decimal {
            precision: 15,
            scale: 2,
        };
        let expr = || {
            Expr::new(
                ExprKind::Arith {
                    op: ArithOp::Add,
                    left: Box::new(Expr::new(ExprKind::Column(0), ty, Pos::START)),
                    right: Box::new(Expr::new(ExprKind::Column(0), ty, Pos::START)),
                },
                SqlType::Decimal {
                    precision: 16,
                    scale: 2,
                },
                Pos::START,
            )
        };
        let a = aggregate(vec![expr(), expr()]);
        let mut p = Program::new(&a);
        let first: ArrayRef = Arc::new(
            Decimal64Array::from(vec![Some(125), None, Some(-225)])
                .with_precision_and_scale(15, 2)
                .expect("decimal"),
        );
        let b = RecordBatch::try_from_iter([("x", first)]).expect("batch");
        let types = [ty];
        let ev = Evaluator::new(&b, &types);
        let (_, args) = p.evaluate(&b, &ev, &a, &[0, 1]).expect("decimal add");
        let out = args[0].as_ref().expect("arg");
        assert_eq!(Atom::get(out, 0), Atom::Dec(250));
        assert_eq!(Atom::get(out, 1), Atom::Null);
        assert_eq!(Atom::get(out, 2), Atom::Null);
        let Data::Dec(v) = &out.data else {
            panic!("decimal")
        };
        let ptr = v.as_ptr();
        drop(args);
        let second: ArrayRef = Arc::new(
            Decimal128Array::from(vec![Some(7), Some(9)])
                .with_precision_and_scale(15, 2)
                .expect("decimal"),
        );
        let b = RecordBatch::try_from_iter([("x", second)]).expect("batch");
        let types = [ty];
        let ev = Evaluator::new(&b, &types);
        let (_, args) = p
            .evaluate(&b, &ev, &a, &[1])
            .expect("reuse wide Arrow input");
        let out = args[0].as_ref().expect("arg");
        assert_eq!(Atom::get(out, 0), Atom::Null);
        assert_eq!(Atom::get(out, 1), Atom::Dec(18));
        let Data::Dec(v) = &out.data else {
            panic!("decimal")
        };
        assert_eq!(v.as_ptr(), ptr);
    }
    #[test]
    fn text_literals_do_not_add_owned_instruction_copies() {
        let a = aggregate(vec![Expr::literal(
            Value::Str("text".into()),
            SqlType::Varchar,
            Pos::START,
        )]);
        let p = Program::new(&a);
        assert!(matches!(p.nodes[0].code, Code::Generic(0)));
    }
    #[test]
    fn complete_conditional_roots_share_without_merging_different_values() {
        let conditional = |value, pos| {
            Expr::new(
                ExprKind::Case {
                    operand: None,
                    whens: vec![(
                        Expr::literal(Value::Bool(false), SqlType::Boolean, pos),
                        binary(ArithOp::IntDiv, lit(10), col(), pos),
                    )],
                    else_: Box::new(lit(value)),
                },
                SqlType::BigInt,
                pos,
            )
        };
        let a = aggregate(vec![
            conditional(3, Pos::START),
            conditional(4, Pos::START),
            conditional(3, Pos { line: 2, col: 9 }),
        ]);
        let mut p = Program::new(&a);
        let b = batch(vec![Some(0)]);
        let types = [SqlType::BigInt];
        let ev = Evaluator::new(&b, &types);
        let (_, args) = p.evaluate(&b, &ev, &a, &[0]).expect("masked conditional");
        let x = args[0].as_ref().expect("arg");
        let y = args[1].as_ref().expect("arg");
        let z = args[2].as_ref().expect("arg");
        assert!(Arc::ptr_eq(x, z));
        assert!(!Arc::ptr_eq(x, y));
        assert_eq!(Atom::get(x, 0), Atom::Int(3));
        assert_eq!(Atom::get(y, 0), Atom::Int(4));
    }
    #[test]
    fn first_positioned_error_and_conditional_phase_match_evaluator() {
        let first = Pos { line: 2, col: 10 };
        let later = Pos { line: 3, col: 20 };
        let a = aggregate(vec![
            binary(ArithOp::IntDiv, lit(10), col(), first),
            binary(ArithOp::IntDiv, lit(10), col(), later),
        ]);
        let b = batch(vec![Some(0), Some(2)]);
        let types = [SqlType::BigInt];
        let ev = Evaluator::new(&b, &types);
        let expected = ev
            .eval(a.aggregates[0].arg.as_ref().expect("arg"), &[0, 1])
            .expect_err("zero");
        assert_eq!(
            Program::new(&a)
                .evaluate(&b, &ev, &a, &[0, 1])
                .expect_err("first error"),
            expected
        );
        let conditional = Expr::new(
            ExprKind::Case {
                operand: None,
                whens: vec![(
                    Expr::literal(Value::Bool(false), SqlType::Boolean, Pos::START),
                    binary(ArithOp::IntDiv, lit(10), col(), later),
                )],
                else_: Box::new(product()),
            },
            SqlType::BigInt,
            Pos::START,
        );
        let conditional2 = Expr::new(
            ExprKind::Case {
                operand: None,
                whens: vec![(
                    Expr::literal(Value::Bool(false), SqlType::Boolean, later),
                    binary(ArithOp::IntDiv, lit(10), col(), first),
                )],
                else_: Box::new(product()),
            },
            SqlType::BigInt,
            later,
        );
        let a = aggregate(vec![conditional, product(), conditional2]);
        let mut p = Program::new(&a);
        assert!(matches!(p.nodes[0].code, Code::Generic(0)));
        let (_, args) = p.evaluate(&b, &ev, &a, &[0, 1]).expect("unselected branch");
        assert!(Arc::ptr_eq(
            args[0].as_ref().expect("arg"),
            args[2].as_ref().expect("arg")
        ));
        assert_eq!(Atom::get(args[0].as_ref().expect("arg"), 0), Atom::Int(0));
    }
}

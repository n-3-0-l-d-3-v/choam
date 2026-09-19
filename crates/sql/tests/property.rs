//! Property tests (ticket 003): the parser never panics on arbitrary
//! text, and `parse(render(ast)) == ast` for arbitrary well-formed trees.

use proptest::prelude::*;
use row::ColumnType;
use sql::*;

fn arb_ident() -> impl Strategy<Value = String> {
    "[a-zA-Z_][a-zA-Z0-9_]{0,6}".prop_filter("not a keyword", |s| !is_keyword(s))
}

fn arb_literal() -> impl Strategy<Value = Literal> {
    prop_oneof![
        Just(Literal::Null),
        (0i64..=i64::MAX).prop_map(Literal::Integer),
        ".{0,8}".prop_map(Literal::Text),
        any::<bool>().prop_map(Literal::Boolean),
    ]
}

fn arb_binop() -> impl Strategy<Value = BinaryOp> {
    prop_oneof![
        Just(BinaryOp::Add),
        Just(BinaryOp::Sub),
        Just(BinaryOp::Mul),
        Just(BinaryOp::Div),
        Just(BinaryOp::Eq),
        Just(BinaryOp::Ne),
        Just(BinaryOp::Lt),
        Just(BinaryOp::Le),
        Just(BinaryOp::Gt),
        Just(BinaryOp::Ge),
        Just(BinaryOp::And),
        Just(BinaryOp::Or),
    ]
}

fn arb_expr() -> impl Strategy<Value = Expr> {
    let leaf = prop_oneof![
        arb_literal().prop_map(Expr::Literal),
        arb_ident().prop_map(Expr::Column),
    ];
    leaf.prop_recursive(4, 32, 4, |inner| {
        prop_oneof![
            (inner.clone(), any::<bool>()).prop_map(|(e, neg)| Expr::Unary(
                if neg { UnaryOp::Neg } else { UnaryOp::Not },
                Box::new(e)
            )),
            (inner.clone(), arb_binop(), inner.clone()).prop_map(|(l, op, r)| Expr::Binary(
                Box::new(l),
                op,
                Box::new(r)
            )),
            (inner, any::<bool>()).prop_map(|(e, n)| Expr::IsNull(Box::new(e), n)),
        ]
    })
}

fn arb_type() -> impl Strategy<Value = ColumnType> {
    prop_oneof![
        Just(ColumnType::Integer),
        Just(ColumnType::Text),
        Just(ColumnType::Bytes),
        Just(ColumnType::Boolean),
    ]
}

fn arb_statement() -> impl Strategy<Value = Statement> {
    let filter = || prop::option::of(arb_expr());
    prop_oneof![
        (
            arb_ident(),
            prop::collection::vec(
                (arb_ident(), arb_type(), any::<bool>(), any::<bool>()).prop_map(
                    |(name, ty, primary_key, not_null)| ColumnSpec {
                        name,
                        ty,
                        primary_key,
                        not_null
                    }
                ),
                1..5
            )
        )
            .prop_map(|(name, columns)| Statement::CreateTable { name, columns }),
        (
            arb_ident(),
            prop::option::of(prop::collection::vec(arb_ident(), 1..4)),
            prop::collection::vec(prop::collection::vec(arb_expr(), 1..4), 1..4)
        )
            .prop_map(|(table, columns, rows)| Statement::Insert {
                table,
                columns,
                rows
            }),
        (
            prop_oneof![
                Just(Projection::All),
                prop::collection::vec(arb_ident(), 1..4).prop_map(Projection::Columns)
            ],
            arb_ident(),
            filter(),
            prop::option::of(
                (arb_ident(), any::<bool>())
                    .prop_map(|(column, descending)| OrderBy { column, descending })
            ),
            prop::option::of(0u64..1_000_000)
        )
            .prop_map(
                |(projection, table, filter, order_by, limit)| Statement::Select {
                    projection,
                    table,
                    filter,
                    order_by,
                    limit
                }
            ),
        (
            arb_ident(),
            prop::collection::vec((arb_ident(), arb_expr()), 1..4),
            filter()
        )
            .prop_map(|(table, assignments, filter)| Statement::Update {
                table,
                assignments,
                filter
            }),
        (arb_ident(), filter()).prop_map(|(table, filter)| Statement::Delete { table, filter }),
        Just(Statement::Begin),
        Just(Statement::Commit),
        Just(Statement::Rollback),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn rendering_then_parsing_round_trips(stmt in arb_statement()) {
        let text = stmt.to_string();
        let parsed = parse(&text);
        prop_assert_eq!(parsed, Ok(stmt), "text was: {}", text);
    }

    #[test]
    fn arbitrary_text_never_panics(s in ".{0,80}") {
        let _ = parse(&s);
        let _ = parse_script(&s);
    }

    #[test]
    fn sql_shaped_garbage_never_panics(
        words in prop::collection::vec(
            prop_oneof![
                Just("SELECT"), Just("FROM"), Just("WHERE"), Just("("), Just(")"), Just(","),
                Just("'"), Just("*"), Just("-"), Just("="), Just("<>"), Just("NULL"), Just("1"),
                Just("x"), Just(";"), Just("INSERT"), Just("VALUES"), Just("NOT"), Just("IS"),
            ],
            0..30
        )
    ) {
        let _ = parse_script(&words.join(" "));
    }
}

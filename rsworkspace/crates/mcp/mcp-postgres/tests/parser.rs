use mcp_postgres::{Error, Query};

#[test]
fn supports_explicit_columns_and_dependencies() -> Result<(), Box<dyn std::error::Error>> {
    for sql in [
        "SELECT id FROM public.customers",
        "SELECT c.id AS customer FROM public.customers c WHERE c.email = 'example' ORDER BY c.id DESC LIMIT 10",
        "SELECT c.id, o.id FROM public.customers c INNER JOIN public.orders o ON c.id = o.customer_id WHERE o.id > 3 AND c.id < 100",
        "SELECT \"odd\"\"name\" FROM \"business schema\".\"table name\"",
    ] {
        Query::parse(sql)?;
    }
    Ok(())
}

#[test]
fn rejects_ambiguous_or_unbounded_query_shapes() {
    for sql in [
        "SELECT id FROM public.customers ORDER BY id FETCH FIRST 2 ROWS WITH TIES",
        "SELECT id FROM public.customers ORDER BY id NULLS FIRST",
        "SELECT id FROM public.customers LIMIT $1",
        "SELECT id FROM public.customers LIMIT '10'",
        "SELECT id FROM public.customers LIMIT -1",
        "SELECT id FROM pg_catalog.pg_class",
        "SELECT id FROM information_schema.tables",
        "SELECT id FROM pg_temp.customers",
        "SELECT a.id FROM public.customers a JOIN public.customers a ON a.id = a.id",
        "SELECT a.id FROM public.customers a RIGHT JOIN public.orders b ON a.id=b.id",
        "SELECT a.id FROM public.customers a FULL JOIN public.orders b ON a.id=b.id",
        "SELECT public.customers.id FROM public.customers",
        "SELECT id FROM public.customers WHERE id = 1.5",
        "SELECT id FROM public.customers WHERE id + 1 = 2",
        "SELECT id FROM public.customers WHERE NOT id = 1",
        "SELECT id FROM public.customers WHERE id = $0",
        "SELECT id FROM public.customers WHERE id = $33",
        "SELECT id FROM public.customers a(id)",
        "SELECT id FROM database.public.customers",
        "SELECT a.id FROM (public.customers a JOIN public.orders b ON a.id=b.id) c",
    ] {
        assert!(matches!(Query::parse(sql), Err(Error::UnsupportedSql)), "{sql}");
    }
}

#[test]
fn bounds_query_size_relation_count_and_expression_depth() -> Result<(), Box<dyn std::error::Error>> {
    let valid = "SELECT id FROM public.customers";
    Query::parse(&format!("{valid}{}", " ".repeat(16_384 - valid.len())))?;
    assert!(matches!(Query::parse(&" ".repeat(16_385)), Err(Error::Limit)));
    Query::parse("SELECT id FROM public.customers LIMIT 0")?;
    Query::parse("SELECT id FROM public.customers LIMIT 1000")?;
    assert!(matches!(
        Query::parse("SELECT id FROM public.customers LIMIT 1001"),
        Err(Error::Limit)
    ));

    let joined = (1..8).fold("public.customers c0".to_owned(), |left, index| {
        format!("{left} JOIN public.customers c{index} ON true")
    });
    Query::parse(&format!("SELECT c0.id FROM {joined}"))?;
    assert!(matches!(
        Query::parse(&format!("SELECT c0.id FROM {joined} JOIN public.customers c8 ON true")),
        Err(Error::Limit)
    ));
    let too_deep = (1..11).fold("public.customers c0".to_owned(), |left, index| {
        format!("{left} JOIN public.customers c{index} ON true")
    });
    assert!(matches!(
        Query::parse(&format!("SELECT c0.id FROM {too_deep}")),
        Err(Error::Limit)
    ));

    let predicate = (0..18).fold("id = 1".to_owned(), |inner, index| {
        if index % 2 == 0 {
            format!("id = 2 AND ({inner})")
        } else {
            format!("id = 3 OR ({inner})")
        }
    });
    assert!(matches!(
        Query::parse(&format!("{valid} WHERE {predicate}")),
        Err(Error::Limit)
    ));
    Ok(())
}

#[test]
fn accepts_scoped_boolean_join_predicates_and_supported_literals() -> Result<(), Box<dyn std::error::Error>> {
    for sql in [
        "SELECT a.id FROM public.customers a JOIN public.orders b ON (a.id=b.id AND b.id=$1) OR false",
        "SELECT id FROM public.customers WHERE id=1 OR id=2 ORDER BY id ASC",
        "SELECT id FROM public.customers WHERE true AND false",
        "SELECT id FROM public.customers WHERE email='can''t'",
    ] {
        Query::parse(sql)?;
    }
    Ok(())
}
#[test]
fn rejects_every_unsupported_construct() {
    for sql in [
        "SELECT a.id FROM public.a a JOIN public.b b ON c.id=b.id JOIN public.c c ON c.id=a.id",
        "SELECT a.id FROM public.a a JOIN public.b b ON id=b.id",
        "SELECT * FROM public.customers",
        "SELECT c.* FROM public.customers c",
        "SELECT 1",
        "SELECT id FROM customers",
        "SELECT id FROM public.customers; SELECT id FROM public.customers",
        "DELETE FROM public.customers",
        "SELECT id INTO public.copy FROM public.customers",
        "SELECT pg_sleep(1) FROM public.customers",
        "SELECT id::text FROM public.customers",
        "SELECT id FROM public.customers FOR UPDATE",
        "WITH x AS (SELECT id FROM public.customers) SELECT id FROM x",
        "SELECT id FROM public.customers WHERE EXISTS (SELECT 1)",
        "SELECT id FROM public.customers GROUP BY id",
        "SELECT DISTINCT id FROM public.customers",
        "SELECT id FROM public.customers UNION SELECT id FROM public.orders",
        "SELECT id FROM public.customers WHERE id OPERATOR(public.=) 1",
        "SELECT id FROM public.customers ORDER BY id USING OPERATOR(public.<)",
        "SELECT id FROM public.customers TABLESAMPLE SYSTEM (10)",
        "SELECT id FROM public.customers OFFSET 5",
        "SELECT id FROM public.customers c NATURAL JOIN public.orders o",
        "SELECT id FROM public.customers c JOIN public.orders o USING(id)",
        "SELECT id FROM public.customers c LEFT JOIN public.orders o ON c.id=o.id",
        "SELECT id FROM public.customers LIMIT ALL",
        "SELECT id FROM public.customers WHERE email COLLATE public.custom = 'x'",
    ] {
        assert!(Query::parse(sql).is_err(), "accepted unsupported SQL: {sql}");
    }
}

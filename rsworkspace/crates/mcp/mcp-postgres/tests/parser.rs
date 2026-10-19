use mcp_postgres::Query;

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

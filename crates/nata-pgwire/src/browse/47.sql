
        SELECT  n.nspname schema_name,
                c.relname table_name
        FROM    pg_catalog.pg_class c
                LEFT JOIN pg_catalog.pg_namespace n
                    ON n.oid = c.relnamespace
        WHERE   c.relkind = ANY($1)
        ORDER BY 1,2;


                    SELECT n.nspname schema_name,
                           t.typname type_name
                    FROM   pg_catalog.pg_type t
                           INNER JOIN pg_catalog.pg_namespace n
                              ON n.oid = t.typnamespace
                    WHERE ( t.typrelid = 0  -- non-composite types
                            OR (  -- composite type, but not a table
                                  SELECT c.relkind = 'c'
                                  FROM pg_catalog.pg_class c
                                  WHERE c.oid = t.typrelid
                                )
                          )
                          AND NOT EXISTS( -- ignore array types
                                SELECT  1
                                FROM    pg_catalog.pg_type el
                                WHERE   el.oid = t.typelem AND el.typarray = t.oid
                              )
                          AND n.nspname <> 'pg_catalog'
                          AND n.nspname <> 'information_schema'
                    ORDER BY 1, 2;

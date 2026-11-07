
                SELECT n.nspname schema_name,
                        p.proname func_name,
                        p.proargnames,
                        COALESCE(proallargtypes::regtype[], proargtypes::regtype[])::text[],
                        p.proargmodes,
                        prorettype::regtype::text return_type,
                        p.prokind = 'a' is_aggregate,
                        p.prokind = 'w' is_window,
                        p.proretset is_set_returning,
                        d.deptype = 'e' is_extension,
                        pg_get_expr(proargdefaults, 0) AS arg_defaults
                FROM pg_catalog.pg_proc p
                        INNER JOIN pg_catalog.pg_namespace n
                            ON n.oid = p.pronamespace
                LEFT JOIN pg_depend d ON d.objid = p.oid and d.deptype = 'e'
                WHERE p.prorettype::regtype != 'trigger'::regtype
                ORDER BY 1, 2

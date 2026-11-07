
                SELECT  nsp.nspname schema_name,
                        cls.relname table_name,
                        att.attname column_name,
                        att.atttypid::regtype::text type_name,
                        att.atthasdef AS has_default,
                        pg_catalog.pg_get_expr(def.adbin, def.adrelid, true) as default
                FROM    pg_catalog.pg_attribute att
                        INNER JOIN pg_catalog.pg_class cls
                            ON att.attrelid = cls.oid
                        INNER JOIN pg_catalog.pg_namespace nsp
                            ON cls.relnamespace = nsp.oid
                        LEFT OUTER JOIN pg_attrdef def
                            ON def.adrelid = att.attrelid
                            AND def.adnum = att.attnum
                WHERE   cls.relkind = ANY($1)
                        AND NOT att.attisdropped
                        AND att.attnum  > 0
                ORDER BY 1, 2, att.attnum

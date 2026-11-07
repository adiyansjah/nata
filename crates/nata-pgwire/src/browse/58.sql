SELECT nspname FROM pg_catalog.pg_namespace WHERE (nspname !~ '^pg_' OR oid = pg_my_temp_schema()) AND nspname <> 'information_schema'

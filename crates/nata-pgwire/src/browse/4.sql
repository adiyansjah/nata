SELECT c.oid,
  n.nspname,
  c.relname
FROM pg_catalog.pg_class c
     LEFT JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
WHERE c.relname OPERATOR(pg_catalog.~) $1 COLLATE pg_catalog.default
  AND n.nspname OPERATOR(pg_catalog.~) $2 COLLATE pg_catalog.default
ORDER BY 2, 3;

SELECT c.oid::pg_catalog.regclass
FROM pg_catalog.pg_class c, pg_catalog.pg_inherits i
WHERE c.oid = i.inhparent AND i.inhrelid = $1
  AND c.relkind != 'p' AND c.relkind != 'I'
ORDER BY inhseqno;

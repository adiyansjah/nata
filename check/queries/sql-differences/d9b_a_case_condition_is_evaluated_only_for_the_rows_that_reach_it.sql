-- expect-nata: id,x
-- expect-nata: 1,ok
-- expect-nata: 2,ok
SELECT id, CASE WHEN id > 0 THEN 'ok' WHEN CAST('abc' AS INTEGER) > 1 THEN 'n' ELSE 'e' END AS x FROM '{data}/sqlcheck/types.parquet' WHERE id <= 2

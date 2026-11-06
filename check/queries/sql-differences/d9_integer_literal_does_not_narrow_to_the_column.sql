-- expect-nata: id,a,b
-- expect-nata: 4,128,65534
SELECT id, ti + 1 AS a, si * 2 AS b FROM '{data}/sqlcheck/types.parquet' WHERE id = 4

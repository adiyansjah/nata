SELECT id, i IS NULL AS a, i IS NOT NULL AS b, (i + 1) IS NULL AS c, s IS NULL AS d, d IS NOT NULL AS e FROM '{data}/sqlcheck/types.parquet' WHERE i < 100 OR i IS NULL

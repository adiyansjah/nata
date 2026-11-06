SELECT id, CASE i WHEN NULL THEN 'n' ELSE 'e' END AS a, CASE WHEN i IS NULL THEN 'null' ELSE 'v' END AS b FROM '{data}/sqlcheck/types.parquet'

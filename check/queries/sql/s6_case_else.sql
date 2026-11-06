SELECT id, CASE WHEN i < 2 THEN 1 ELSE 2 END AS a FROM '{data}/sqlcheck/types.parquet'

SELECT id, CASE WHEN i > 2 THEN CASE WHEN i > 5 THEN 'a' ELSE 'b' END ELSE 'c' END AS x FROM '{data}/sqlcheck/types.parquet'

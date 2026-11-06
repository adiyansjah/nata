SELECT id, CASE WHEN id < 5 THEN dec52 WHEN id < 9 THEN dec183 ELSE 1 END AS a FROM '{data}/sqlcheck/types.parquet'

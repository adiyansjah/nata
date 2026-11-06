SELECT id, CASE WHEN id < 5 THEN ti ELSE b END AS a FROM '{data}/sqlcheck/types.parquet'

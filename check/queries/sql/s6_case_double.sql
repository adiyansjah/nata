SELECT id, CASE WHEN id < 4 THEN d ELSE CAST(0 AS DOUBLE) END AS a FROM '{data}/sqlcheck/types.parquet'

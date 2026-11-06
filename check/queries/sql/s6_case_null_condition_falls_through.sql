SELECT id, CASE WHEN i > 2 THEN 'big' ELSE 'small-or-null' END AS a FROM '{data}/sqlcheck/types.parquet'

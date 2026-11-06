-- expect-error
SELECT CASE WHEN i = 0 THEN 1 // 0 ELSE 5 END AS a FROM '{data}/sqlcheck/types.parquet'

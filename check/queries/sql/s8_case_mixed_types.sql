-- expect-error
SELECT CASE WHEN f THEN 1 ELSE 'a' END AS a FROM '{data}/sqlcheck/types.parquet'

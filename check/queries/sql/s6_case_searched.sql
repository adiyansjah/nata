SELECT id, CASE WHEN i < 0 THEN 'neg' WHEN i = 0 THEN 'zero' WHEN i > 0 THEN 'pos' END AS a FROM '{data}/sqlcheck/types.parquet'

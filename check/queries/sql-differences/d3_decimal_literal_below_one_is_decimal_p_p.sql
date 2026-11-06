-- tolerance: 0
SELECT 0.1 AS a, .5 AS c, 0.000 AS f, -0.25 AS g FROM '{data}/sqlcheck/types.parquet' LIMIT 1

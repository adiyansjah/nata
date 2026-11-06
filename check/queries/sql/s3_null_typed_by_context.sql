SELECT i + NULL AS a, NULL = i AS b, CAST(NULL AS DATE) AS c FROM '{data}/sqlcheck/types.parquet'

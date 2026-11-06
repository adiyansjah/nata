SELECT id, i BETWEEN NULL AND 5 AS a, i BETWEEN 1 AND NULL AS b, i NOT BETWEEN NULL AND 0 AS c FROM '{data}/sqlcheck/types.parquet'

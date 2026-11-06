SELECT id, i IN (1, NULL) AS a, i NOT IN (1, NULL) AS b, i IN (NULL) AS c, 2 IN (1, NULL) AS d FROM '{data}/sqlcheck/types.parquet'

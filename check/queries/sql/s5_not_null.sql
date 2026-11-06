SELECT id, NOT (i = NULL) AS a, NOT (NULL AND f) AS b FROM '{data}/sqlcheck/types.parquet'

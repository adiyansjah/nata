SELECT id, d != CAST(1.5 AS DOUBLE) AS v, d != d AS w FROM '{data}/sqlcheck/types.parquet'

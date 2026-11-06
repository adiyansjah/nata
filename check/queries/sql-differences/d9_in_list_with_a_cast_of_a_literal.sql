-- expect-error-nata: not a literal in an IN list
SELECT id, d IN (CAST(1.5 AS DOUBLE)) AS a FROM '{data}/sqlcheck/types.parquet'

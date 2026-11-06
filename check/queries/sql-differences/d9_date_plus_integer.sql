-- expect-error-nata: arithmetic on dates
SELECT dt + 1 AS a FROM '{data}/sqlcheck/types.parquet'

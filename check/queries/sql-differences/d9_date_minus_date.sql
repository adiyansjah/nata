-- expect-error-nata: arithmetic on dates
SELECT dt - dt AS a FROM '{data}/sqlcheck/types.parquet'

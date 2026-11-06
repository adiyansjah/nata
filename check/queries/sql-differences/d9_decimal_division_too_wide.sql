-- expect-error-nata: digits
SELECT dec3810 / dec3810 AS a FROM '{data}/sqlcheck/types.parquet'

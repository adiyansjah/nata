-- expect-error
SELECT dec3810 + dec33 AS a FROM '{data}/sqlcheck/types.parquet' WHERE id = 4

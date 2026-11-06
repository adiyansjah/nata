-- expect-error
SELECT dec183 * dec183 AS a FROM '{data}/sqlcheck/types.parquet' WHERE id = 4

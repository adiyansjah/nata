-- scan: {data}/nation.parquet --filter 'n_comment IS NOT NULL' --filter 'n_regionkey <> 2'
SELECT * FROM '{data}/nation.parquet' WHERE n_comment IS NOT NULL AND n_regionkey <> 2

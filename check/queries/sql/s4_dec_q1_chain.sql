SELECT id, dec52 * (1 - dec33) * (1 + dec33) AS v FROM '{data}/sqlcheck/types.parquet' WHERE id IN (1,2,3,7,8,9,12)

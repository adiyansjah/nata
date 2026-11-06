SELECT id, dec52 + dec52 AS a, dec52 - dec183 AS b, dec52 * dec52 AS c, dec183 * dec52 AS d FROM '{data}/sqlcheck/types.parquet' WHERE id IN (1,2,3,6,7,9,10)

SELECT id, dec52 % 1.7 AS a, dec183 % dec52 AS b, i % dec52 AS c FROM '{data}/sqlcheck/types.parquet' WHERE id IN (2,3,7,8,12)

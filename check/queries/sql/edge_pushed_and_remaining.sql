SELECT id, dt FROM '{data}/sqlcheck/types.parquet' WHERE dt >= DATE '1994-01-01' AND dt < DATE '2001-01-01' AND i % 5 > 0

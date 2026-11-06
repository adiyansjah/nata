-- expect-nata: id,x
-- expect-nata: 1,true
-- expect-nata: 2,false
SELECT id, (i = 0 OR 10 // i > 100) AS x FROM '{data}/sqlcheck/types.parquet' WHERE id IN (1, 2)

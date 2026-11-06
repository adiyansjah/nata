-- expect-nata: id,x
-- expect-nata: 1,false
-- expect-nata: 2,true
SELECT id, (i <> 0 AND 10 // i > 1) AS x FROM '{data}/sqlcheck/types.parquet' WHERE id IN (1, 2)

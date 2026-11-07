-- expect-nata: v
-- expect-nata: 3.333333
SELECT avg(i) AS v FROM '{data}/sqlcheck/types.parquet' WHERE id IN (2, 7, 12);

-- expect-nata: v
-- expect-nata: 2147483648
SELECT sum(i) + 1 AS v FROM '{data}/sqlcheck/types.parquet' WHERE id = 4;

-- expect-nata: v
-- expect-nata: 9223372036854775808
SELECT sum(b) + 1 AS v FROM '{data}/sqlcheck/types.parquet' WHERE id = 4;

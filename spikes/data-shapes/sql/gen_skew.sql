-- (1) SKEWED keys. @N@ fact rows, @K@ distinct keys per key column.
--   k10   Zipf-like, exponent 1.0     (rank 1 holds about ln2/ln(K) of the rows, a long tail)
--   k15   Zipf-like, exponent 1.5     (rank 1 holds about 29 percent)
--   khot  one hot key (0) holding exactly 30 percent of the rows in expectation, the rest uniform over 1..K
--   kuni  uniform over 0..K-1         (the control: same query shape, no skew)
--   l_shipmode  a low-cardinality STRING key (20 values, Zipf 1.0). The name makes the hash-ops reader decode it as a
--                dictionary column (src/source.rs), so the dictionary key path sees a skewed key.
COPY (
  SELECT i AS id,
         zipf10(u01(i, 1), @K@) AS k10,
         zipf15(u01(i, 2), @K@) AS k15,
         CASE WHEN u01(i, 3) < 0.30 THEN 0::BIGINT ELSE 1 + ri(i, 4, @K@) END AS khot,
         ri(i, 5, @K@) AS kuni,
         ri(i, 6, 1000) AS v,
         ri(i, 7, 1000001) - 500000 AS w,
         'MODE_' || lpad(zipf10(u01(i, 8), 20)::VARCHAR, 2, '0') AS l_shipmode
  FROM range(@N@) t(i)
) TO '@DIR@/skew_fact.parquet' (FORMAT PARQUET, ROW_GROUP_SIZE 122880);

-- The dimension: every key 0..K once. df = dk % 2 lets a join drop the hot key (df = 1) or keep it (df = 0).
-- o_orderpriority is a dictionary column on the build side (payload).
COPY (
  SELECT k AS dk, k % 1000 AS dg, k % 97 AS dp, k % 2 AS df,
         'P' || (k % 5)::VARCHAR AS o_orderpriority
  FROM range(@K@ + 1) t(k)
) TO '@DIR@/skew_dim.parquet' (FORMAT PARQUET, ROW_GROUP_SIZE 122880);

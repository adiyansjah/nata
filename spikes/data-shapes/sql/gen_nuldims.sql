-- Join partners for the NULL sets, NULL keys on the BUILD side too:
--   nul_dimn  keys 0..K-1 once plus @K10@ rows with a NULL key
--   nul_sdim  100 distinct strings ks0..ks99, 5 copies of '', 5 rows with a NULL key
COPY (
  SELECT CASE WHEN k < @K@ THEN k ELSE NULL END::BIGINT AS dk, k % 97 AS dpay
  FROM range(@K@ + @K10@) t(k)
) TO '@DIR@/nul_dimn.parquet' (FORMAT PARQUET, ROW_GROUP_SIZE 122880);
COPY (
  SELECT CASE WHEN k < 100 THEN 'ks' || k::VARCHAR WHEN k < 105 THEN '' ELSE NULL END AS dks, k % 7 AS dsp
  FROM range(110) t(k)
) TO '@DIR@/nul_sdim.parquet' (FORMAT PARQUET, ROW_GROUP_SIZE 122880);

-- (1b) The hot key on the BUILD side: a chain of 30 percent of the build rows. Output size is chain length x number
-- of probe rows with that key, so the probe side has exactly 100 hot rows (a quadratic bug shows as time growing
-- faster than the output; the harness prints ns per output row for the small and the big build side).
--   hot_build  @B@ rows, bk = 0 for 30 percent of the rows, else uniform 1..B;  bid = row number
--   hot_probe  @PROBE@ rows, pk = 0 for exactly 100 rows (every P/100-th), else uniform 1..B
--   hh_a, hh_b @HH@ rows each, 30 percent key 0 on BOTH sides (hot x hot: (0.3 HH)^2 output rows)
COPY (
  SELECT i AS bid, CASE WHEN u01(i, 1) < 0.30 THEN 0::BIGINT ELSE 1 + ri(i, 2, @B@) END AS bk,
         ri(i, 3, 1000) AS bv, i % 7 AS bg
  FROM range(@B@) t(i)
) TO '@DIR@/hot_build.parquet' (FORMAT PARQUET, ROW_GROUP_SIZE 122880);
COPY (
  SELECT i AS pid, CASE WHEN i % (@PROBE@ // 100) = 0 THEN 0::BIGINT ELSE 1 + ri(i, 5, @B@) END AS pk,
         ri(i, 6, 1000) AS pv, i % 11 AS pg
  FROM range(@PROBE@) t(i)
) TO '@DIR@/hot_probe.parquet' (FORMAT PARQUET, ROW_GROUP_SIZE 122880);
COPY (
  SELECT i AS aid, CASE WHEN u01(i, 11) < 0.30 THEN 0::BIGINT ELSE 1 + ri(i, 12, @HH@) END AS ak,
         ri(i, 13, 1000) AS av, i % 10 AS ag
  FROM range(@HH@) t(i)
) TO '@DIR@/hh_a.parquet' (FORMAT PARQUET, ROW_GROUP_SIZE 122880);
COPY (
  SELECT i AS cid, CASE WHEN u01(i, 21) < 0.30 THEN 0::BIGINT ELSE 1 + ri(i, 22, @HH@) END AS bk2,
         ri(i, 23, 1000) AS bv2, i % 10 AS bg2
  FROM range(@HH@) t(i)
) TO '@DIR@/hh_b.parquet' (FORMAT PARQUET, ROW_GROUP_SIZE 122880);

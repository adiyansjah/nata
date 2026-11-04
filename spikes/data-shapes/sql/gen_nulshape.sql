-- (3b) The SHAPE of the NULLs, not only their rate. @NS@ rows, row groups of exactly @RGS@ rows (a multiple of
-- DuckDB's vector size 2048, so groups are cut at @RGS@ x g).
--   kb_al  NULL for whole aligned blocks of 8192 rows (the engine's batch size): batches that are ALL NULL
--   kb_un  NULL for blocks of 5000 rows: all-NULL stretches that straddle batch boundaries
--   kr     NULL in EVERY row group but one: non-NULL only in the middle half of row group number @TG@
--   ksb    string key, block NULLs like kb_al      vb  value, block NULLs      vr  value NULL in every group but one
--   kall / vall  NULL in every row (BIGINT)
--   g      id % 10, never NULL        v    never NULL
COPY (
  SELECT i AS id,
         CASE WHEN (i // 8192) % 3 = 0 THEN NULL ELSE ri(i, 1, 1000) END AS kb_al,
         CASE WHEN (i // 5000) % 3 = 1 THEN NULL ELSE ri(i, 2, 1000) END AS kb_un,
         CASE WHEN i >= @RGS@ * @TG@ + @RGS@ // 4 AND i < @RGS@ * @TG@ + @RGS@ * 3 // 4 THEN ri(i, 3, 1000) ELSE NULL END AS kr,
         CASE WHEN (i // 8192) % 3 = 2 THEN NULL ELSE 's' || ri(i, 4, 1000)::VARCHAR END AS ksb,
         CASE WHEN (i // 8192) % 2 = 0 THEN NULL ELSE ri(i, 5, 1000) END AS vb,
         CASE WHEN i >= @RGS@ * @TG@ + @RGS@ // 4 AND i < @RGS@ * @TG@ + @RGS@ * 3 // 4 THEN ri(i, 6, 1000) ELSE NULL END AS vr,
         NULL::BIGINT AS kall,
         NULL::BIGINT AS vall,
         i % 10 AS g,
         ri(i, 7, 1000) AS v
  FROM range(@NS@) t(i)
) TO '@DIR@/nul_shape.parquet' (FORMAT PARQUET, ROW_GROUP_SIZE @RGS@);
